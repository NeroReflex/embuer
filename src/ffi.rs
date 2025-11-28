/*
    embuer: an embedded software updater DBUS daemon and CLI interface
    Copyright (C) 2025  Denis Benato
    
    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.
    
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.
    
    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::ptr;
use tokio::runtime::Runtime;
use zbus::Connection;

use crate::dbus::EmbuerDBusProxy;

/// Opaque handle to the Embuer client context
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct embuer_client_t {
    runtime: Runtime,
    connection: Connection,
}

/// Status callback function type
/// Parameters: status, details, progress, user_data
pub type StatusCallback =
    unsafe extern "C" fn(*const c_char, *const c_char, c_int, *mut std::ffi::c_void);

/// Error codes
pub const EMBUER_OK: c_int = 0;
pub const EMBUER_ERR_NULL_PTR: c_int = -1;
pub const EMBUER_ERR_CONNECTION: c_int = -2;
pub const EMBUER_ERR_DBUS: c_int = -3;
pub const EMBUER_ERR_INVALID_STRING: c_int = -4;
pub const EMBUER_ERR_RUNTIME: c_int = -5;
pub const EMBUER_ERR_NO_PENDING_UPDATE: c_int = -6;

/// Initialize a new Embuer client
/// Returns a handle to the client or NULL on error
#[no_mangle]
pub unsafe extern "C" fn embuer_client_new() -> *mut embuer_client_t {
    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return ptr::null_mut(),
    };

    let connection = match runtime.block_on(async { Connection::system().await }) {
        Ok(conn) => conn,
        Err(_) => return ptr::null_mut(),
    };

    let client = Box::new(embuer_client_t {
        runtime,
        connection,
    });

    Box::into_raw(client)
}

/// Free an Embuer client
#[no_mangle]
pub unsafe extern "C" fn embuer_client_free(client: *mut embuer_client_t) {
    if !client.is_null() {
        let _ = Box::from_raw(client);
    }
}

/// Get the boot deployment information
///
/// Parameters:
/// - client: Client handle
/// - boot_id_out: Pointer to store boot ID
/// - boot_name_out: Pointer to store boot name string (must be freed with embuer_free_string)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_get_boot_info(
    client: *mut embuer_client_t,
    boot_id_out: *mut u64,
    boot_name_out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || boot_id_out.is_null() || boot_name_out.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.get_boot_info().await
    });

    match result {
        Ok((boot_id, boot_name)) => {
            let boot_name_c = match CString::new(boot_name) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *boot_id_out = boot_id;
                *boot_name_out = boot_name_c.into_raw();
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_DBUS,
    }
}

/// Get the current update status
///
/// Parameters:
/// - client: Client handle
/// - status_out: Pointer to store status string (must be freed with embuer_free_string)
/// - details_out: Pointer to store details string (must be freed with embuer_free_string)
/// - progress_out: Pointer to store progress value (0-100, or -1 if N/A)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_get_status(
    client: *mut embuer_client_t,
    status_out: *mut *mut c_char,
    details_out: *mut *mut c_char,
    progress_out: *mut c_int,
) -> c_int {
    if client.is_null() || status_out.is_null() || details_out.is_null() || progress_out.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.get_update_status().await
    });

    match result {
        Ok((status, details, progress)) => {
            let status_c = match CString::new(status) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            let details_c = match CString::new(details) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *status_out = status_c.into_raw();
                *details_out = details_c.into_raw();
                *progress_out = progress;
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_DBUS,
    }
}

/// Install an update from a file
///
/// Parameters:
/// - client: Client handle
/// - file_path: Path to the update file
/// - result_out: Pointer to store result message (must be freed with embuer_free_string)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_install_from_file(
    client: *mut embuer_client_t,
    file_path: *const c_char,
    result_out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || file_path.is_null() || result_out.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };
    let path_str = unsafe {
        match CStr::from_ptr(file_path).to_str() {
            Ok(s) => s,
            Err(_) => return EMBUER_ERR_INVALID_STRING,
        }
    };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.install_update_from_file(path_str.to_string()).await
    });

    match result {
        Ok(msg) => {
            let msg_c = match CString::new(msg) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *result_out = msg_c.into_raw();
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_DBUS,
    }
}

/// Install an update from a URL
///
/// Parameters:
/// - client: Client handle
/// - url: URL to download the update from
/// - result_out: Pointer to store result message (must be freed with embuer_free_string)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_install_from_url(
    client: *mut embuer_client_t,
    url: *const c_char,
    result_out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || url.is_null() || result_out.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };
    let url_str = unsafe {
        match CStr::from_ptr(url).to_str() {
            Ok(s) => s,
            Err(_) => return EMBUER_ERR_INVALID_STRING,
        }
    };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.install_update_from_url(url_str.to_string()).await
    });

    match result {
        Ok(msg) => {
            let msg_c = match CString::new(msg) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *result_out = msg_c.into_raw();
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_DBUS,
    }
}

/// Free a string allocated by the library
#[no_mangle]
pub unsafe extern "C" fn embuer_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

/// Get the pending update awaiting confirmation
///
/// Parameters:
/// - client: Client handle
/// - version_out: Pointer to store version string (must be freed with embuer_free_string)
/// - changelog_out: Pointer to store changelog string (must be freed with embuer_free_string)
/// - source_out: Pointer to store source string (must be freed with embuer_free_string)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_get_pending_update(
    client: *mut embuer_client_t,
    version_out: *mut *mut c_char,
    changelog_out: *mut *mut c_char,
    source_out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || version_out.is_null() || changelog_out.is_null() || source_out.is_null()
    {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.get_pending_update().await
    });

    match result {
        Ok((version, changelog, source)) => {
            let version_c = match CString::new(version) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            let changelog_c = match CString::new(changelog) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            let source_c = match CString::new(source) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *version_out = version_c.into_raw();
                *changelog_out = changelog_c.into_raw();
                *source_out = source_c.into_raw();
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_NO_PENDING_UPDATE,
    }
}

/// Confirm or reject the pending update
///
/// Parameters:
/// - client: Client handle
/// - accepted: 1 to accept and install, 0 to reject
/// - result_out: Pointer to store result message (must be freed with embuer_free_string)
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_confirm_update(
    client: *mut embuer_client_t,
    accepted: c_int,
    result_out: *mut *mut c_char,
) -> c_int {
    if client.is_null() || result_out.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };
    let accepted_bool = accepted != 0;

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;
        proxy.confirm_update(accepted_bool).await
    });

    match result {
        Ok(msg) => {
            let msg_c = match CString::new(msg) {
                Ok(s) => s,
                Err(_) => return EMBUER_ERR_INVALID_STRING,
            };

            unsafe {
                *result_out = msg_c.into_raw();
            }

            EMBUER_OK
        }
        Err(_) => EMBUER_ERR_DBUS,
    }
}

/// Watch for status updates (blocking call)
/// This function will block and call the callback whenever the status changes
///
/// Parameters:
/// - client: Client handle
/// - callback: Function to call on status updates
/// - user_data: User data to pass to the callback
///
/// Returns: EMBUER_OK on success, error code otherwise
#[no_mangle]
pub unsafe extern "C" fn embuer_watch_status(
    client: *mut embuer_client_t,
    callback: StatusCallback,
    user_data: *mut std::ffi::c_void,
) -> c_int {
    if client.is_null() {
        return EMBUER_ERR_NULL_PTR;
    }

    let client = unsafe { &*client };

    let result = client.runtime.block_on(async {
        let proxy = EmbuerDBusProxy::new(&client.connection).await?;

        // Get initial status
        let (status, details, progress) = proxy.get_update_status().await?;

        let status_c =
            CString::new(status).map_err(|_| zbus::Error::Failure("Invalid string".to_string()))?;
        let details_c = CString::new(details)
            .map_err(|_| zbus::Error::Failure("Invalid string".to_string()))?;

        callback(status_c.as_ptr(), details_c.as_ptr(), progress, user_data);

        // Subscribe to status change signals
        let mut stream = proxy.receive_update_status_changed().await?;

        use futures_util::StreamExt;
        while let Some(signal) = stream.next().await {
            let args = signal.args()?;

            let status_c = CString::new(args.status)
                .map_err(|_| zbus::Error::Failure("Invalid string".to_string()))?;
            let details_c = CString::new(args.details)
                .map_err(|_| zbus::Error::Failure("Invalid string".to_string()))?;

            callback(
                status_c.as_ptr(),
                details_c.as_ptr(),
                args.progress,
                user_data,
            );
        }

        Ok::<(), zbus::Error>(())
    });

    match result {
        Ok(_) => EMBUER_OK,
        Err(_) => EMBUER_ERR_DBUS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_lifecycle() {
        let client = unsafe { embuer_client_new() };
        assert!(!client.is_null());
        unsafe { embuer_client_free(client) };
    }

    #[test]
    fn test_null_safety() {
        let mut status_out = ptr::null_mut();
        let mut details_out = ptr::null_mut();
        let mut progress_out = 0;

        let result = unsafe {
            embuer_get_status(
                ptr::null_mut(),
                &mut status_out,
                &mut details_out,
                &mut progress_out,
            )
        };

        assert_eq!(result, EMBUER_ERR_NULL_PTR);
    }
}
