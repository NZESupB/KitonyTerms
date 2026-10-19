//! Windows 凭据窗口只收集系统账户凭据，必须登录成功且 SID 匹配当前进程账户。

use std::{ffi::c_void, mem::size_of, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, ERROR_INSUFFICIENT_BUFFER, HANDLE},
    Security::{
        Credentials::{
            CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CREDUIWIN_GENERIC,
            CREDUI_INFOW,
        },
        EqualSid, GetTokenInformation, LogonUserW, TokenUser, LOGON32_LOGON_INTERACTIVE,
        LOGON32_PROVIDER_DEFAULT, TOKEN_QUERY, TOKEN_USER,
    },
    System::{
        Com::CoTaskMemFree,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};
use zeroize::{Zeroize, Zeroizing};

use super::AuthError;

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

// TOKEN_USER 包含指针，因此使用 usize 存储保证对齐，并保持其内嵌 SID 缓冲存活。
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
        return Err(AuthError::Failed);
    }
    Ok(storage)
}

fn same_account(token: &Token) -> Result<(), AuthError> {
    let mut current = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current) } == 0 {
        return Err(AuthError::Failed);
    }
    let current = Token(current);
    let left = token_user(&current)?;
    let right = token_user(token)?;
    let equal = unsafe {
        EqualSid(
            (*(left.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            (*(right.as_ptr().cast::<TOKEN_USER>())).User.Sid,
        )
    };
    if equal == 0 {
        Err(AuthError::Failed)
    } else {
        Ok(())
    }
}

pub(super) fn authenticate(reason: &str, owner: isize) -> Result<(), AuthError> {
    let title: Vec<u16> = "KitonyTerms\0".encode_utf16().collect();
    let reason: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    let info = CREDUI_INFOW {
        cbSize: size_of::<CREDUI_INFOW>() as u32,
        hwndParent: owner as _,
        pszMessageText: reason.as_ptr(),
        pszCaptionText: title.as_ptr(),
        hbmBanner: ptr::null_mut(),
    };
    let mut package = 0;
    let mut save = 0;
    let mut credentials = PackedCredentials {
        pointer: ptr::null_mut(),
        length: 0,
    };
    let status = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut package,
            ptr::null(),
            0,
            &mut credentials.pointer,
            &mut credentials.length,
            &mut save,
            CREDUIWIN_GENERIC,
        )
    };
    if status == ERROR_CANCELLED {
        return Err(AuthError::Cancelled);
    }
    if status != 0 || credentials.pointer.is_null() {
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
        );
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || user_len == 0
        || user_len > 4096
        || domain_len > 4096
        || password_len == 0
        || password_len > 4096
    {
        return Err(AuthError::Failed);
    }
    let mut user = Zeroizing::new(vec![0u16; user_len as usize]);
    let mut domain = Zeroizing::new(vec![0u16; domain_len.max(1) as usize]);
    let mut password = Zeroizing::new(vec![0u16; password_len as usize]);
    domain_len = domain.len() as u32;
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
        return Err(AuthError::Failed);
    }
    if let Some(separator) = user
        .iter()
        .position(|&character| character == u16::from(b'\\'))
    {
        domain = Zeroizing::new(user[..separator].iter().copied().chain(Some(0)).collect());
        user = Zeroizing::new(user[separator + 1..].to_vec());
    }
    // 明确验证交互式登录凭据，不把“点击了系统窗口的确定”当成认证成功。
    let mut token = ptr::null_mut();
    let domain_pointer = if domain[0] == 0 || user.contains(&u16::from(b'@')) {
        ptr::null()
    } else {
        domain.as_ptr()
    };
    if unsafe {
        LogonUserW(
            user.as_ptr(),
            domain_pointer,
            password.as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        )
    } == 0
    {
        return Err(AuthError::Failed);
    }
    same_account(&Token(token))
}
