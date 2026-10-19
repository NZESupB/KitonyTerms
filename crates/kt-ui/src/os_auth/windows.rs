//! Windows Hello 与系统账户密码验证；密码路径必须登录成功且 SID 匹配当前进程。

use super::{AuthError, AuthMethod};
use ::windows::{
    core::{factory, HSTRING},
    Security::Credentials::UI::{
        UserConsentVerificationResult as Verification, UserConsentVerifier,
        UserConsentVerifierAvailability as Availability,
    },
    Win32::{
        Foundation::HWND,
        System::WinRT::{
            IUserConsentVerifierInterop, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
        },
    },
};
use std::{ffi::c_void, mem::size_of, ptr};
use windows_future::IAsyncOperation;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCOUNT_LOCKED_OUT, ERROR_ACCOUNT_RESTRICTION,
        ERROR_CANCELLED, ERROR_INSUFFICIENT_BUFFER, ERROR_LOGON_FAILURE,
        ERROR_LOGON_TYPE_NOT_GRANTED, HANDLE,
    },
    Security::{
        Credentials::{
            CredPackAuthenticationBufferW, CredUIPromptForWindowsCredentialsW,
            CredUnPackAuthenticationBufferW, CREDUIWIN_GENERIC, CREDUI_INFOW,
            CRED_PACK_GENERIC_CREDENTIALS,
        },
        EqualSid, GetTokenInformation, LogonUserW, LookupAccountSidW, TokenUser,
        LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, TOKEN_QUERY, TOKEN_USER,
    },
    System::{
        Com::CoTaskMemFree,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};
use zeroize::{Zeroize, Zeroizing};

type Wide = Zeroizing<Vec<u16>>;
fn wide(value: &str) -> Wide {
    Zeroizing::new(value.encode_utf16().chain(Some(0)).collect())
}
fn failed(stage: &'static str, code: u32) -> AuthError {
    tracing::warn!(stage, code, "Windows 系统认证失败");
    match code {
        ERROR_CANCELLED => AuthError::Cancelled,
        ERROR_LOGON_FAILURE => AuthError::WrongCredentials,
        ERROR_ACCOUNT_LOCKED_OUT => AuthError::AccountLocked,
        ERROR_ACCOUNT_RESTRICTION | ERROR_LOGON_TYPE_NOT_GRANTED => AuthError::AccountRestricted,
        _ => AuthError::Failed,
    }
}

struct Token(HANDLE);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
struct PackedCredentials {
    pointer: *mut c_void,
    length: u32,
}
impl Drop for PackedCredentials {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.pointer.cast::<u8>(), self.length as usize)
                    .zeroize();
                CoTaskMemFree(self.pointer);
            }
        }
    }
}

fn current_token() -> Result<Token, AuthError> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(failed("process_token", unsafe { GetLastError() }));
    }
    Ok(Token(token))
}

// TOKEN_USER 包含指针，使用 usize 存储保证对齐并保持内嵌 SID 缓冲存活。
fn token_user(token: &Token) -> Result<Vec<usize>, AuthError> {
    let mut length = 0;
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length) };
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || !(size_of::<TOKEN_USER>() as u32..=65536).contains(&length)
    {
        return Err(AuthError::Failed);
    }
    let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(failed("token_user", unsafe { GetLastError() }));
    }
    Ok(storage)
}

fn same_account(token: &Token) -> Result<(), AuthError> {
    let left = token_user(&current_token()?)?;
    let right = token_user(token)?;
    let equal = unsafe {
        EqualSid(
            (*(left.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            (*(right.as_ptr().cast::<TOKEN_USER>())).User.Sid,
        )
    };
    if equal == 0 {
        Err(AuthError::WrongCredentials)
    } else {
        Ok(())
    }
}

fn current_account() -> Result<(Wide, Wide), AuthError> {
    let token = current_token()?;
    let storage = token_user(&token)?;
    let sid = unsafe { (*(storage.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let (mut name_len, mut domain_len, mut usage) = (0, 0, 0);
    unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            ptr::null_mut(),
            &mut name_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut usage,
        )
    };
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || name_len == 0
        || name_len > 4096
        || domain_len > 4096
    {
        return Err(AuthError::Failed);
    }
    let mut name = Zeroizing::new(vec![0u16; name_len as usize]);
    let mut domain = Zeroizing::new(vec![0u16; domain_len.max(1) as usize]);
    domain_len = domain.len() as u32;
    if unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut usage,
        )
    } == 0
    {
        return Err(failed("account_lookup", unsafe { GetLastError() }));
    }
    Ok((name, domain))
}

struct RoGuard;
impl Drop for RoGuard {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

enum HelloDecision {
    Verified,
    UsePassword,
    Cancelled,
    Busy,
    Denied,
}
fn verify_hello(reason: &str, owner: isize) -> ::windows::core::Result<HelloDecision> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
    let _guard = RoGuard;
    match UserConsentVerifier::CheckAvailabilityAsync()?.join()? {
        Availability::Available => {}
        Availability::DeviceBusy => return Ok(HelloDecision::Busy),
        _ => return Ok(HelloDecision::UsePassword),
    }
    let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
    let operation: IAsyncOperation<Verification> = unsafe {
        interop
            .RequestVerificationForWindowAsync(HWND(owner as *mut c_void), &HSTRING::from(reason))?
    };
    Ok(match operation.join()? {
        Verification::Verified => HelloDecision::Verified,
        Verification::Canceled => HelloDecision::Cancelled,
        Verification::DeviceBusy => HelloDecision::Busy,
        _ => HelloDecision::Denied,
    })
}

pub(super) fn authenticate(
    reason: &str,
    owner: isize,
    method: AuthMethod,
) -> Result<(), AuthError> {
    if owner == 0 {
        return Err(AuthError::Unavailable);
    }
    if method == AuthMethod::Auto {
        match verify_hello(reason, owner) {
            Ok(HelloDecision::Verified) => return Ok(()),
            Ok(HelloDecision::Cancelled) => return Err(AuthError::Cancelled),
            Ok(HelloDecision::Busy) => return Err(AuthError::Busy),
            Ok(HelloDecision::Denied) => return Err(AuthError::Failed),
            Ok(HelloDecision::UsePassword) => {}
            Err(error) => {
                let code = error.code().0 as u32;
                // 只允许明确的接口/类不可用或系统不支持回退；不吞掉认证和公寓错误。
                if !matches!(code, 0x80004002 | 0x80040154 | 0x80040111 | 0x80070032) {
                    return Err(failed("hello", code));
                }
            }
        }
    }
    verify_credentials(reason, owner)
}

fn decode_wide(value: &[u16]) -> Result<Zeroizing<String>, AuthError> {
    let end = value
        .iter()
        .position(|&c| c == 0)
        .ok_or(AuthError::Failed)?;
    String::from_utf16(&value[..end])
        .map(Zeroizing::new)
        .map_err(|_| AuthError::Failed)
}

fn pack_account(name: &[u16], domain: &[u16]) -> Result<Zeroizing<Vec<u8>>, AuthError> {
    let name = decode_wide(name)?;
    let domain = decode_wide(domain)?;
    let username = if domain.is_empty() {
        wide(&name)
    } else {
        wide(&format!("{}\\{}", domain.as_str(), name.as_str()))
    };
    let mut length = 0;
    unsafe {
        CredPackAuthenticationBufferW(
            CRED_PACK_GENERIC_CREDENTIALS,
            username.as_ptr(),
            [0u16].as_ptr(),
            ptr::null_mut(),
            &mut length,
        )
    };
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || length == 0 || length > 65536 {
        return Err(AuthError::Failed);
    }
    let mut input = Zeroizing::new(vec![0u8; length as usize]);
    if unsafe {
        CredPackAuthenticationBufferW(
            CRED_PACK_GENERIC_CREDENTIALS,
            username.as_ptr(),
            [0u16].as_ptr(),
            input.as_mut_ptr(),
            &mut length,
        )
    } == 0
    {
        return Err(failed("pack_account", unsafe { GetLastError() }));
    }
    input.truncate(length as usize);
    Ok(input)
}

fn verify_credentials(reason: &str, owner: isize) -> Result<(), AuthError> {
    let (account_name, account_domain) = current_account()?;
    let input = pack_account(&account_name, &account_domain)?;
    let title = wide("KitonyTerms");
    let reason = wide(reason);
    let info = CREDUI_INFOW {
        cbSize: size_of::<CREDUI_INFOW>() as u32,
        hwndParent: owner as _,
        pszMessageText: reason.as_ptr(),
        pszCaptionText: title.as_ptr(),
        hbmBanner: ptr::null_mut(),
    };
    let (mut package, mut save) = (0, 0);
    let mut credentials = PackedCredentials {
        pointer: ptr::null_mut(),
        length: 0,
    };
    let status = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut package,
            input.as_ptr().cast(),
            input.len() as u32,
            &mut credentials.pointer,
            &mut credentials.length,
            &mut save,
            CREDUIWIN_GENERIC,
        )
    };
    if status != 0 {
        return Err(failed("credential_prompt", status));
    }
    if credentials.pointer.is_null() || credentials.length == 0 {
        return Err(AuthError::Failed);
    }
    let (mut user_len, mut domain_len, mut password_len) = (0, 0, 0);
    unsafe {
        CredUnPackAuthenticationBufferW(
            0,
            credentials.pointer,
            credentials.length,
            ptr::null_mut(),
            &mut user_len,
            ptr::null_mut(),
            &mut domain_len,
            ptr::null_mut(),
            &mut password_len,
        )
    };
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || user_len == 0
        || user_len > 4096
        || domain_len > 4096
        || password_len > 4096
    {
        return Err(AuthError::Failed);
    }
    let mut user = Zeroizing::new(vec![0u16; user_len as usize]);
    let mut domain = Zeroizing::new(vec![0u16; domain_len.max(1) as usize]);
    // 长度包含结尾 NUL；空密码传 [0]，不以空字符串推断账户设置或绕过验证。
    let mut password = Zeroizing::new(vec![0u16; password_len.max(1) as usize]);
    domain_len = domain.len() as u32;
    password_len = password.len() as u32;
    if unsafe {
        CredUnPackAuthenticationBufferW(
            0,
            credentials.pointer,
            credentials.length,
            user.as_mut_ptr(),
            &mut user_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            password.as_mut_ptr(),
            &mut password_len,
        )
    } == 0
    {
        return Err(failed("unpack_credentials", unsafe { GetLastError() }));
    }
    let user_text = decode_wide(&user)?;
    let domain_text = decode_wide(&domain)?;
    let current_name = decode_wide(&account_name)?;
    let current_domain = decode_wide(&account_domain)?;
    let (user, domain) = account_parts(&user_text, &domain_text, &current_name, &current_domain);
    verify_password(
        &user,
        domain.as_deref().map(|value| value.as_slice()),
        &password,
    )
}

fn account_parts(
    user: &str,
    domain: &str,
    current_name: &str,
    current_domain: &str,
) -> (Wide, Option<Wide>) {
    if let Some((domain, user)) = user.split_once('\\') {
        return (wide(user), Some(wide(domain)));
    }
    if user.contains('@') {
        return (wide(user), None);
    }
    let domain = if !domain.is_empty() {
        domain
    } else if user.eq_ignore_ascii_case(current_name) && !current_domain.is_empty() {
        current_domain
    } else {
        "."
    };
    (wide(user), Some(wide(domain)))
}

fn verify_password(
    user: &[u16],
    domain: Option<&[u16]>,
    password: &[u16],
) -> Result<(), AuthError> {
    if user.is_empty()
        || user[0] == 0
        || user.last() != Some(&0)
        || password.last() != Some(&0)
        || domain.is_some_and(|d| d.is_empty() || d.last() != Some(&0))
    {
        return Err(AuthError::Failed);
    }
    let mut token = ptr::null_mut();
    if unsafe {
        LogonUserW(
            user.as_ptr(),
            domain.map_or(ptr::null(), |d| d.as_ptr()),
            password.as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        )
    } == 0
    {
        return Err(failed("logon", unsafe { GetLastError() }));
    }
    same_account(&Token(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_names_keep_domain_and_upn_semantics() {
        let (name, domain) = account_parts("user", "", "user", "PC");
        assert_eq!(decode_wide(&name).unwrap().as_str(), "user");
        assert_eq!(
            decode_wide(domain.as_ref().unwrap()).unwrap().as_str(),
            "PC"
        );
        assert!(account_parts("user@example.org", "", "user", "PC")
            .1
            .is_none());
        let (name, domain) = account_parts(".\\user", "", "user", "PC");
        assert_eq!(decode_wide(&name).unwrap().as_str(), "user");
        assert_eq!(decode_wide(domain.as_ref().unwrap()).unwrap().as_str(), ".");
        assert_eq!(
            failed("test", ERROR_ACCOUNT_RESTRICTION),
            AuthError::AccountRestricted
        );
        assert_eq!(
            failed("test", ERROR_ACCOUNT_LOCKED_OUT),
            AuthError::AccountLocked
        );
    }
}
