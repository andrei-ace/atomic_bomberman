//! Native first-run UI; only the worker owns importer processes and staging.
use crate::native;
use anyhow::{ensure, Result};
use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

type Handle = *mut c_void;
type WindowProc = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;
#[repr(C)]
#[derive(Default)]
struct WindowClass {
    style: u32,
    procedure: Option<WindowProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu: *const u16,
    name: *const u16,
}
#[repr(C)]
#[derive(Default)]
struct Message {
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: [i32; 2],
    private: u32,
}
#[repr(C)]
#[derive(Default)]
struct OpenFile {
    size: u32,
    owner: Handle,
    instance: Handle,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_title: u32,
    initial_directory: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    extension_offset: u16,
    default_extension: *const u16,
    custom_data: isize,
    hook: *const c_void,
    template: *const u16,
    reserved: *mut c_void,
    reserved_word: u32,
    flags_ex: u32,
}
#[repr(C)]
struct BrowseInfo {
    owner: Handle,
    root: *const c_void,
    display: *mut u16,
    title: *const u16,
    flags: u32,
    callback: *const c_void,
    parameter: isize,
    image: i32,
}
#[repr(C)]
struct CommonControls {
    size: u32,
    classes: u32,
}
#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        extended: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: *const c_void,
    ) -> Handle;
    fn DefWindowProcW(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowLongPtrW(window: Handle, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(window: Handle, index: i32) -> isize;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn LoadIconW(instance: Handle, name: *const u16) -> Handle;
    fn ShowWindow(window: Handle, command: i32) -> i32;
    fn GetMessageW(message: *mut Message, window: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(message: *const Message) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn IsDialogMessageW(window: Handle, message: *const Message) -> i32;
    fn DestroyWindow(window: Handle) -> i32;
    fn PostQuitMessage(code: i32);
    fn SetWindowTextW(window: Handle, text: *const u16) -> i32;
    fn EnableWindow(window: Handle, enabled: i32) -> i32;
    fn SendMessageW(window: Handle, message: u32, wparam: usize, lparam: isize) -> isize;
    fn SetTimer(window: Handle, id: usize, milliseconds: u32, callback: *const c_void) -> usize;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetStockObject(object: i32) -> Handle;
}
#[link(name = "comctl32")]
unsafe extern "system" {
    fn InitCommonControlsEx(controls: *const CommonControls) -> i32;
}
#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(file: *mut OpenFile) -> i32;
    fn CommDlgExtendedError() -> u32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHBrowseForFolderW(info: *const BrowseInfo) -> *mut c_void;
    fn SHGetPathFromIDListEx(item: *const c_void, path: *mut u16, size: u32, flags: u32) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
    fn CoUninitialize();
    fn CoTaskMemFree(memory: *mut c_void);
}

fn text(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

struct State {
    root: PathBuf,
    bin: PathBuf,
    status: Handle,
    progress: Handle,
    choose_file: Handle,
    choose_folder: Handle,
    cancel_button: Handle,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    receiver: Option<mpsc::Receiver<Result<PathBuf>>>,
    close_after_cancel: bool,
    result: Option<PathBuf>,
}
impl State {
    fn status(&self, value: &str) {
        unsafe {
            SetWindowTextW(self.status, text(value).as_ptr());
        }
    }
    fn busy(&self, busy: bool) {
        unsafe {
            EnableWindow(self.choose_file, i32::from(!busy));
            EnableWindow(self.choose_folder, i32::from(!busy));
            EnableWindow(self.cancel_button, 1);
            SetWindowTextW(
                self.cancel_button,
                text(if busy { "Cancel" } else { "Quit" }).as_ptr(),
            );
            ShowWindow(self.progress, if busy { 5 } else { 0 });
            SendMessageW(self.progress, 0x40a, usize::from(busy), 50);
        }
    }
    fn start(&mut self, source: PathBuf) {
        self.cancel.store(false, Ordering::Release);
        let root = self.root.clone();
        let bin = self.bin.clone();
        let cancel = self.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.worker = Some(thread::spawn(move || {
            let _ = sender.send(native::import(&root, &bin, &source, &cancel));
        }));
        self.busy(true);
        self.status("Importing and validating game data. This can take several minutes.");
    }
    fn stop(&mut self, window: Handle, close: bool) {
        if self.worker.is_some() {
            self.close_after_cancel |= close;
            self.cancel.store(true, Ordering::Release);
            self.status("Cancelling import. Your previous game data is preserved.");
            unsafe {
                EnableWindow(self.cancel_button, 0);
            }
        } else {
            unsafe {
                DestroyWindow(window);
            }
        }
    }
    fn poll(&mut self, window: Handle) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!(
                "Import worker stopped. Previous game data is preserved."
            )),
        };
        self.receiver = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if self.close_after_cancel {
            unsafe {
                DestroyWindow(window);
            }
            return;
        }
        match result {
            Ok(path) => {
                self.result = Some(path);
                unsafe {
                    DestroyWindow(window);
                }
            }
            Err(error) => {
                self.busy(false);
                self.status(&format!("{error:#}"));
            }
        }
    }
}

unsafe extern "system" fn procedure(
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == 0x81 {
        // CREATESTRUCTW.lpCreateParams is its first member.
        SetWindowLongPtrW(window, -21, *(lparam as *const isize));
    }
    let state = GetWindowLongPtrW(window, -21) as *mut State;
    if state.is_null() {
        return DefWindowProcW(window, message, wparam, lparam);
    }
    match message {
        0x111 => {
            match wparam & 0xffff {
                1 | 4 if (*state).worker.is_none() => match choose(window, wparam & 0xffff == 4) {
                    Ok(Some(path)) => (*state).start(path),
                    Ok(None) => (),
                    Err(error) => (*state).status(&format!("{error:#}")),
                },
                2 => (*state).stop(window, false),
                _ => (),
            }
            0
        }
        0x113 => {
            (*state).poll(window);
            0
        }
        0x10 => {
            (*state).stop(window, true);
            0
        }
        2 => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn control(
    window: Handle,
    class: &str,
    title: &str,
    id: usize,
    rect: [i32; 4],
    extra: u32,
) -> Result<Handle> {
    let [x, y, width, height] = rect;
    let handle = CreateWindowExW(
        0,
        text(class).as_ptr(),
        text(title).as_ptr(),
        0x50000000 | extra,
        x,
        y,
        width,
        height,
        window,
        id as Handle,
        GetModuleHandleW(ptr::null()),
        ptr::null(),
    );
    ensure!(
        !handle.is_null(),
        "create launcher control: {}",
        std::io::Error::last_os_error()
    );
    SendMessageW(handle, 0x30, GetStockObject(17) as usize, 1);
    Ok(handle)
}

pub fn choose_data(root: &Path, bin: &Path) -> Result<Option<PathBuf>> {
    let mut state = Box::new(State {
        root: root.into(),
        bin: bin.into(),
        status: ptr::null_mut(),
        progress: ptr::null_mut(),
        choose_file: ptr::null_mut(),
        choose_folder: ptr::null_mut(),
        cancel_button: ptr::null_mut(),
        cancel: Arc::new(AtomicBool::new(false)),
        worker: None,
        receiver: None,
        close_after_cancel: false,
        result: None,
    });
    unsafe {
        ensure!(
            CoInitializeEx(ptr::null(), 2) >= 0,
            "initialize Windows file chooser"
        );
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _com = Com;
        let controls = CommonControls {
            size: std::mem::size_of::<CommonControls>() as u32,
            classes: 0x20,
        };
        ensure!(
            InitCommonControlsEx(&controls) != 0,
            "initialize Windows controls"
        );
        let class_name = text("AtomicBombermanImporter");
        let class = WindowClass {
            procedure: Some(procedure),
            instance: GetModuleHandleW(ptr::null()),
            cursor: LoadCursorW(ptr::null_mut(), 32512usize as *const u16),
            icon: LoadIconW(ptr::null_mut(), 32512usize as *const u16),
            background: 6usize as Handle,
            name: class_name.as_ptr(),
            ..Default::default()
        };
        ensure!(RegisterClassW(&class) != 0, "register launcher window");
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            text("Atomic Bomberman — Choose Game Data").as_ptr(),
            0x00ca0000,
            i32::MIN,
            i32::MIN,
            680,
            330,
            ptr::null_mut(),
            ptr::null_mut(),
            class.instance,
            (&mut *state as *mut State).cast(),
        );
        ensure!(!window.is_null(), "create launcher window");
        control(window, "STATIC", "Choose your original .7z archive, .img/.iso disc image, or extracted game folder.\r\nYour original files stay unchanged. Import is needed only once.", 0, [24, 24, 620, 60], 0)?;
        state.status = control(
            window,
            "STATIC",
            "Original game data is required. You can also repair an existing import.",
            0,
            [24, 92, 620, 90],
            0,
        )?;
        state.progress = control(window, "msctls_progress32", "", 0, [24, 184, 620, 16], 8)?;
        state.choose_file = control(
            window,
            "BUTTON",
            "Choose Archive or Disc…",
            1,
            [24, 220, 230, 36],
            0x10000,
        )?;
        state.choose_folder = control(
            window,
            "BUTTON",
            "Choose Folder…",
            4,
            [266, 220, 210, 36],
            0x10000,
        )?;
        state.cancel_button = control(window, "BUTTON", "Quit", 2, [488, 220, 156, 36], 0x10000)?;
        state.busy(false);
        ensure!(
            SetTimer(window, 1, 100, ptr::null()) != 0,
            "start launcher progress timer"
        );
        ShowWindow(window, 5);
        let mut message = Message::default();
        let result = loop {
            let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
            if result <= 0 {
                break result;
            }
            if IsDialogMessageW(window, &message) == 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        };
        state.cancel.store(true, Ordering::Release);
        if let Some(worker) = state.worker.take() {
            let _ = worker.join();
        }
        ensure!(result == 0, "launcher message loop failed");
    }
    Ok(state.result.take())
}

unsafe fn choose(window: Handle, folder: bool) -> Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0u16; 32768];
    let chosen = if folder {
        let title = text("Choose the game folder containing COLOR.PAL and DATA");
        let mut display = vec![0; 260];
        let info = BrowseInfo {
            owner: window,
            root: ptr::null(),
            display: display.as_mut_ptr(),
            title: title.as_ptr(),
            flags: 1 | 0x40 | 0x200,
            callback: ptr::null(),
            parameter: 0,
            image: 0,
        };
        let item = SHBrowseForFolderW(&info);
        if item.is_null() {
            return Ok(None);
        }
        let success = SHGetPathFromIDListEx(item, buffer.as_mut_ptr(), buffer.len() as u32, 0);
        CoTaskMemFree(item);
        ensure!(success != 0, "selected folder has no filesystem path");
        true
    } else {
        let filter =
            text("Original game data (*.7z;*.img;*.iso)\0*.7z;*.img;*.iso\0All files\0*.*\0");
        let title = text("Choose Original Game Data");
        let mut file = OpenFile {
            size: std::mem::size_of::<OpenFile>() as u32,
            owner: window,
            filter: filter.as_ptr(),
            filter_index: 1,
            file: buffer.as_mut_ptr(),
            max_file: buffer.len() as u32,
            title: title.as_ptr(),
            flags: 0x80000 | 0x1000 | 0x800 | 8,
            ..Default::default()
        };
        let success = GetOpenFileNameW(&mut file) != 0;
        if !success {
            ensure!(
                CommDlgExtendedError() == 0,
                "Windows could not open the file chooser"
            );
        }
        success
    };
    if !chosen {
        return Ok(None);
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Ok(Some(
        std::ffi::OsString::from_wide(&buffer[..length]).into(),
    ))
}
