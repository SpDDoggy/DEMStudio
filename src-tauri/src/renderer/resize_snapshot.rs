//! UI-thread-owned GDI cover retaining the last opaque GL frame during resize.
use std::{marker::PhantomData, rc::Rc, sync::OnceLock};
use windows::{core::w, Win32::{Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*}};

#[derive(Default)]
struct Frame { width: i32, height: i32, pixels: Vec<u8> }

unsafe fn paint_frame(dc: HDC, rect: RECT, frame: &Frame) {
    if frame.pixels.is_empty() { return; }
    let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: frame.width, biHeight: frame.height, biPlanes: 1, biBitCount: 32,
        biCompression: BI_RGB.0, ..Default::default()
    }, ..Default::default() };
    SetStretchBltMode(dc, COLORONCOLOR);
    StretchDIBits(dc, 0, 0, rect.right-rect.left, rect.bottom-rect.top,
        0, 0, frame.width, frame.height, Some(frame.pixels.as_ptr().cast()),
        &info, DIB_RGB_COLORS, SRCCOPY);
}

unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let frame = Box::into_raw(Box::<Frame>::default());
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, frame as isize);
            LRESULT(1)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Frame;
            if !ptr.is_null() {
                let mut rect = RECT::default();
                if GetClientRect(hwnd, &mut rect).is_ok() { paint_frame(dc, rect, &*ptr); }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Frame;
            if !ptr.is_null() { drop(Box::from_raw(ptr)); }
            DefWindowProcW(hwnd,message,wp,lp)
        }
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => DefWindowProcW(hwnd,message,wp,lp),
    }
}

/// Must be created, updated and dropped on the same UI thread.
/// Source bytes are BGRA, bottom-up, with an opaque display result.
pub(crate) struct ResizeSnapshot {
    hwnd: HWND,
    parent: HWND,
    _ui_thread: PhantomData<Rc<()>>,
}
impl ResizeSnapshot {
    pub(crate) fn new(parent: HWND, _viewport: HWND) -> Result<Self,String> {
        unsafe {
            static CLASS: OnceLock<Result<(),String>> = OnceLock::new();
            CLASS.get_or_init(|| {
                let module = GetModuleHandleW(None).map_err(|e|e.to_string())?;
                let class = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: HINSTANCE(module.0),
                    lpszClassName: w!("DEMStudioResizeSnapshot"), ..Default::default() };
                if RegisterClassW(&class)==0 && GetLastError()!=ERROR_CLASS_ALREADY_EXISTS {
                    return Err(windows::core::Error::from_thread().to_string());
                }
                Ok(())
            }).clone()?;
            let module = GetModuleHandleW(None).map_err(|e|e.to_string())?;
            // WS_VISIBLE is deliberately absent; publishing a complete frame precedes showing.
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("DEMStudioResizeSnapshot"), w!(""),
                WS_CHILD,0,0,1,1,Some(parent),None,Some(HINSTANCE(module.0)),None);
            let hwnd = match hwnd { Ok(hwnd)=>hwnd, Err(e)=> {
                return Err(e.to_string());
            }};
            let this = Self { hwnd,parent,_ui_thread: PhantomData };
            this.resize()?;
            Ok(this)
        }
    }
    pub(crate) fn show(&self, width:u32, height:u32, pixels:Vec<u8>) -> Result<(),String> {
        let length = (width as usize).checked_mul(height as usize).and_then(|n|n.checked_mul(4));
        if width==0 || height==0 || width>i32::MAX as u32 || height>i32::MAX as u32 || length!=Some(pixels.len()) {
            return Err("无效的窗口保留帧".into());
        }
        unsafe {
            let frame = &mut *(GetWindowLongPtrW(self.hwnd,GWLP_USERDATA) as *mut Frame);
            *frame = Frame { width:width as i32,height:height as i32,pixels };
            self.resize()?;
            let webview = FindWindowExW(Some(self.parent),None,w!("WRY_WEBVIEW"),None).ok();
            SetWindowPos(self.hwnd,Some(webview.unwrap_or(HWND_TOP)),0,0,0,0,
                SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE|SWP_SHOWWINDOW).map_err(|e|e.to_string())?;
            let _ = RedrawWindow(Some(self.hwnd),None,None,RDW_INVALIDATE|RDW_UPDATENOW|RDW_NOERASE);
        }
        Ok(())
    }
    pub(crate) fn resize(&self) -> Result<(),String> {
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(self.parent,&mut rect).map_err(|e|e.to_string())?;
            SetWindowPos(self.hwnd,None,0,0,(rect.right-rect.left).max(1),(rect.bottom-rect.top).max(1),
                SWP_NOACTIVATE|SWP_NOZORDER).map_err(|e|e.to_string())?;
            let _ = RedrawWindow(Some(self.hwnd),None,None,RDW_INVALIDATE|RDW_UPDATENOW|RDW_NOERASE);
            Ok(())
        }
    }
    pub(crate) fn hide(&self) { unsafe {
        let _=ShowWindow(self.hwnd,SW_HIDE);
        let ptr=GetWindowLongPtrW(self.hwnd,GWLP_USERDATA) as *mut Frame;
        if !ptr.is_null() { *ptr=Frame::default(); }
    } }
}
impl Drop for ResizeSnapshot { fn drop(&mut self) { unsafe { let _=DestroyWindow(self.hwnd); } } }
