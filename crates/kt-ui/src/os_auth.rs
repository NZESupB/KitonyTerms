//! 查看保存密码前的系统身份认证。成功凭据只能在本模块构造，且只能消费一次。

use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthError {
    Cancelled,
    Unavailable,
    Failed,
}

pub(crate) struct Verified {
    _private: (),
}

static AUTH_PENDING: AtomicBool = AtomicBool::new(false);
struct PendingGuard;
impl Drop for PendingGuard {
    fn drop(&mut self) {
        AUTH_PENDING.store(false, Ordering::Release);
    }
}

/// `owner` 是 Windows 原生窗口句柄，其余平台忽略。不接收 SSH 密码。
pub(crate) async fn authenticate(reason: String, owner: isize) -> Result<Verified, AuthError> {
    AUTH_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| AuthError::Unavailable)?;
    let guard = PendingGuard;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        authenticate_platform(&reason, owner).map(|()| Verified { _private: () })
    })
    .await
    .map_err(|_| AuthError::Failed)?
}

#[cfg(not(any(target_os = "macos", windows)))]
fn authenticate_platform(_reason: &str, _owner: isize) -> Result<(), AuthError> {
    Err(AuthError::Unavailable)
}

#[cfg(windows)]
fn authenticate_platform(reason: &str, owner: isize) -> Result<(), AuthError> {
    windows::authenticate(reason, owner)
}

#[cfg(target_os = "macos")]
fn authenticate_platform(reason: &str, _owner: isize) -> Result<(), AuthError> {
    use block2::RcBlock;
    use objc2::{rc::autoreleasepool, runtime::Bool};
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    autoreleasepool(|_| {
        // 独立上下文不复用先前的认证成功，设备所有者策略允许系统密码回退。
        let context = unsafe { LAContext::new() };
        unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) }
            .map_err(|_| AuthError::Unavailable)?;
        let reason = NSString::from_str(reason);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                let code = unsafe { error.as_ref() }.map(|error| error.code());
                Err(if matches!(code, Some(-2 | -4 | -9)) {
                    AuthError::Cancelled
                } else {
                    AuthError::Failed
                })
            };
            let _ = sender.send(result);
        });
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthentication,
                &reason,
                &reply,
            );
        }
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap_or(Err(AuthError::Cancelled));
        unsafe { context.invalidate() };
        result
    })
}
