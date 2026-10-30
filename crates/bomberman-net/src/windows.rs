//! Windows per-user storage and durable, private profile publication.
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::FromRawHandle,
    },
    path::{Path, PathBuf},
    ptr,
};

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        id: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *mut c_void);
    fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
    fn CoUninitialize();
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        attributes: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
}

pub fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut text: Vec<_> = value.encode_wide().collect();
    if text.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    text.push(0);
    Ok(text)
}

pub fn data_directory() -> io::Result<PathBuf> {
    let initialized = unsafe { CoInitializeEx(ptr::null(), 2) };
    if initialized < 0 && initialized as u32 != 0x80010106 {
        return Err(io::Error::other(format!(
            "Initialize Known Folders: 0x{initialized:08x}"
        )));
    }
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }
    // RPC_E_CHANGED_MODE means COM is already initialized in another mode.
    let _com = Com(initialized >= 0);
    let id = Guid(
        0xf1b32785,
        0x6fba,
        0x4fcf,
        [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
    );
    let mut path = ptr::null_mut();
    let result = unsafe { SHGetKnownFolderPath(&id, 0, ptr::null_mut(), &mut path) };
    if result < 0 || path.is_null() {
        return Err(io::Error::other(format!(
            "LocalAppData lookup failed: 0x{result:08x}"
        )));
    }
    let directory = unsafe {
        let length = (0..32768).find(|&i| *path.add(i) == 0);
        let result =
            length.map(|n| PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(path, n))));
        CoTaskMemFree(path.cast());
        result.ok_or_else(|| io::Error::other("LocalAppData path is too long"))?
    };
    Ok(directory.join("Atomic Bomberman"))
}

pub fn is_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

pub fn create_private_file(path: &Path) -> io::Result<File> {
    let path = wide(path.as_os_str())?;
    // Protected DACL: the owner, administrators and SYSTEM only. Never inherit
    // a broad ACL from an explicitly selected configuration directory.
    let acl = wide(OsStr::new("D:P(A;;FA;;;OW)(A;;FA;;;BA)(A;;FA;;;SY)"))?;
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            acl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor,
        inherit: 0,
    };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0x40000000,
            0,
            &attributes,
            1,
            0x80,
            ptr::null_mut(),
        )
    };
    let error = io::Error::last_os_error();
    unsafe {
        LocalFree(descriptor);
    }
    if handle as isize == -1 {
        Err(error)
    } else {
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide(source.as_os_str())?;
    let destination = wide(destination.as_os_str())?;
    // MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH. Only files are
    // replaced; published asset directories remain immutable.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 1 | 8) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
