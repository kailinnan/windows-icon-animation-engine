#![cfg_attr(not(windows), allow(dead_code))]
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("This program currently supports 64-bit Windows only.");

#[cfg(all(windows, target_pointer_width = "64"))]
mod app {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::fmt;
    use std::mem;
    use std::ptr::{self, NonNull};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    type HResult = i32;
    type ComMethod = *const c_void;

    const S_FALSE: HResult = 1;
    const E_NOTIMPL: HResult = 0x80004001u32 as i32;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const CLSCTX_LOCAL_SERVER: u32 = 0x4;
    const RPC_E_CHANGED_MODE: HResult = 0x80010106u32 as i32;
    const SWC_DESKTOP: i32 = 8;
    const SWFO_NEEDDISPATCH: i32 = 1;
    const SVGIO_ALL: u32 = 0;
    const SVSI_POSITIONITEM: u32 = 0x80;
    const FWF_AUTOARRANGE: u32 = 0x1;
    const FWF_SNAPTOGRID: u32 = 0x4;
    const ARRANGEMENT_FLAGS: u32 = FWF_AUTOARRANGE | FWF_SNAPTOGRID;
    const FVO_CUSTOMPOSITION: u32 = 0x2;
    const ICON_DIAMETER: f64 = 56.0;
    const GRAVITY: f64 = 1800.0;
    const FRAME_TIME: Duration = Duration::from_millis(16);
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const SIGDN_DESKTOPABSOLUTEPARSING: u32 = 0x8002_8000;
    const RECYCLE_BIN_GUID: &str = "645ff040-5081-101b-9f08-00aa002f954e";
    const HOTKEY_ID: i32 = 1;
    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_NOREPEAT: u32 = 0x4000;
    const VK_Q: u32 = 0x51;
    const WM_HOTKEY: u32 = 0x0312;

    const CLSID_SHELL_WINDOWS: Guid = Guid::new(
        0x9ba05972,
        0xf6a8,
        0x11cf,
        [0xa4, 0x42, 0x00, 0xa0, 0xc9, 0x0a, 0x8f, 0x39],
    );
    const IID_ISHELL_WINDOWS: Guid = Guid::new(
        0x85cb6900,
        0x4d95,
        0x11cf,
        [0x96, 0x0c, 0x00, 0x80, 0xc7, 0xf4, 0xee, 0x85],
    );
    const IID_ISERVICE_PROVIDER: Guid = Guid::new(
        0x6d5140c1,
        0x7436,
        0x11ce,
        [0x80, 0x34, 0x00, 0xaa, 0x00, 0x60, 0x09, 0xfa],
    );
    const SID_S_TOP_LEVEL_BROWSER: Guid = Guid::new(
        0x4c96be40,
        0x915c,
        0x11cf,
        [0x99, 0xd3, 0x00, 0xaa, 0x00, 0x4a, 0xe8, 0x37],
    );
    const IID_ISHELL_BROWSER: Guid = Guid::new(
        0x000214e2,
        0x0000,
        0x0000,
        [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    );
    const IID_IFOLDER_VIEW2: Guid = Guid::new(
        0x1af3a467,
        0x214f,
        0x4298,
        [0x90, 0x8e, 0x06, 0xb0, 0x3e, 0x0b, 0x39, 0xf9],
    );
    const IID_IFOLDER_VIEW_OPTIONS: Guid = Guid::new(
        0x3cc974d2,
        0xb302,
        0x4d36,
        [0xad, 0x3e, 0x06, 0xd9, 0x3f, 0x69, 0x5d, 0x3f],
    );
    const IID_IENUM_ID_LIST: Guid = Guid::new(
        0x000214f2,
        0x0000,
        0x0000,
        [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    );

    static RUNNING: AtomicBool = AtomicBool::new(true);
    static RESTORED: AtomicBool = AtomicBool::new(false);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    impl Guid {
        /// 根据四个标准字段构造一个与 Windows ABI 布局一致的 GUID。
        const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
            Self {
                data1,
                data2,
                data3,
                data4,
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    #[repr(C)]
    struct Message {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        point: Point,
        private: u32,
    }

    // A zeroed VT_EMPTY VARIANT. VARIANT is 24 bytes on Win64.
    #[repr(C)]
    struct EmptyVariant([u64; 3]);

    #[derive(Debug)]
    pub(crate) struct Error(String);

    impl fmt::Display for Error {
        /// 将内部错误消息写入格式化器，供控制台错误输出使用。
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    type Result<T> = std::result::Result<T, Error>;

    /// 检查 COM 返回的 HRESULT，失败时附加当前操作名称并转换为本程序的错误类型。
    fn check(hr: HResult, operation: &str) -> Result<()> {
        if hr >= 0 {
            Ok(())
        } else {
            Err(Error(format!(
                "{operation} failed (HRESULT 0x{:08X})",
                hr as u32
            )))
        }
    }

    struct ComApartment(bool);

    impl ComApartment {
        /// 将当前线程初始化为单线程 COM 单元，Shell 接口必须在该单元存活期间使用。
        fn init() -> Result<Self> {
            let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) };
            if hr == RPC_E_CHANGED_MODE {
                return Err(Error(
                    "the current thread already uses an incompatible COM apartment".into(),
                ));
            }
            check(hr, "CoInitializeEx")?;
            Ok(Self(true))
        }
    }

    impl Drop for ComApartment {
        /// 在线程退出作用域时成对调用 CoUninitialize，释放 COM 单元资源。
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct ComPtr(NonNull<c_void>);

    impl ComPtr {
        /// 接管一个 COM 原始接口指针，并拒绝 COM 异常返回的空指针。
        ///
        /// 调用方必须保证该指针拥有一个尚未释放的 COM 引用计数。
        unsafe fn from_raw(raw: *mut c_void, operation: &str) -> Result<Self> {
            NonNull::new(raw)
                .map(Self)
                .ok_or_else(|| Error(format!("{operation} returned a null interface")))
        }

        /// 返回被封装的 COM 接口原始指针，用于调用虚函数表中的方法。
        fn as_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }

        /// 调用 IUnknown::QueryInterface 获取指定 IID 对应的接口，并接管新引用。
        fn query_interface(&self, iid: &Guid, operation: &str) -> Result<Self> {
            type Fn =
                unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult;
            let mut out = ptr::null_mut();
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.as_ptr(), 0)) };
            check(unsafe { call(self.as_ptr(), iid, &mut out) }, operation)?;
            unsafe { Self::from_raw(out, operation) }
        }
    }

    impl Drop for ComPtr {
        /// 调用 IUnknown::Release 释放当前封装持有的一次 COM 引用。
        fn drop(&mut self) {
            type Fn = unsafe extern "system" fn(*mut c_void) -> u32;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.as_ptr(), 2)) };
            unsafe { call(self.as_ptr()) };
        }
    }

    struct Pidl(NonNull<c_void>);

    impl Pidl {
        /// 返回 PIDL 的只读原始地址，供 IFolderView 方法识别对应桌面项目。
        fn as_ptr(&self) -> *const c_void {
            self.0.as_ptr()
        }
    }

    impl Drop for Pidl {
        /// 使用 COM 任务分配器释放 IEnumIDList 返回的 PIDL 内存。
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0.as_ptr()) };
        }
    }

    struct Icon {
        pidl: Pidl,
        original: Point,
        is_recycle_bin: bool,
    }

    struct Body {
        x: f64,
        y: f64,
        top_x: f64,
        top_y: f64,
        velocity_x: f64,
        velocity_y: f64,
        release_at: f64,
        released: bool,
        bounds: Rect,
    }

    #[derive(Clone, Copy)]
    enum AnimationMode {
        GravityBounce,
        RecycleBinCatch,
    }

    impl AnimationMode {
        /// 返回当前模式之后应当执行的另一个动画模式。
        fn next(self) -> Self {
            match self {
                Self::RecycleBinCatch => Self::GravityBounce,
                Self::GravityBounce => Self::RecycleBinCatch,
            }
        }
    }

    struct FolderView {
        view: ComPtr,
        options: ComPtr,
        scratch_pidls: RefCell<Vec<*const c_void>>,
        scratch_points: RefCell<Vec<Point>>,
    }

    impl FolderView {
        /// 沿 ShellWindows → IServiceProvider → IShellBrowser → IShellView 链路连接桌面，
        /// 最终取得控制图标与视图选项所需的 IFolderView2 和 IFolderViewOptions。
        fn desktop() -> Result<Self> {
            let mut windows = ptr::null_mut();
            check(
                unsafe {
                    CoCreateInstance(
                        &CLSID_SHELL_WINDOWS,
                        ptr::null_mut(),
                        CLSCTX_LOCAL_SERVER,
                        &IID_ISHELL_WINDOWS,
                        &mut windows,
                    )
                },
                "creating ShellWindows",
            )?;
            let shell_windows = unsafe { ComPtr::from_raw(windows, "creating ShellWindows")? };

            // IShellWindows::FindWindowSW is vtable slot 15 (after IDispatch).
            type FindFn = unsafe extern "system" fn(
                *mut c_void,
                *const EmptyVariant,
                *const EmptyVariant,
                i32,
                *mut i32,
                i32,
                *mut *mut c_void,
            ) -> HResult;
            let find: FindFn = unsafe { mem::transmute(vtable_entry(shell_windows.as_ptr(), 15)) };
            let empty = EmptyVariant([0; 3]);
            let mut hwnd = 0i32;
            let mut dispatch = ptr::null_mut();
            check(
                unsafe {
                    find(
                        shell_windows.as_ptr(),
                        &empty,
                        &empty,
                        SWC_DESKTOP,
                        &mut hwnd,
                        SWFO_NEEDDISPATCH,
                        &mut dispatch,
                    )
                },
                "finding the desktop Shell window",
            )?;
            let dispatch =
                unsafe { ComPtr::from_raw(dispatch, "finding the desktop Shell window")? };
            let provider =
                dispatch.query_interface(&IID_ISERVICE_PROVIDER, "querying IServiceProvider")?;

            // IServiceProvider::QueryService.
            type ServiceFn = unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                *const Guid,
                *mut *mut c_void,
            ) -> HResult;
            let query_service: ServiceFn =
                unsafe { mem::transmute(vtable_entry(provider.as_ptr(), 3)) };
            let mut browser = ptr::null_mut();
            check(
                unsafe {
                    query_service(
                        provider.as_ptr(),
                        &SID_S_TOP_LEVEL_BROWSER,
                        &IID_ISHELL_BROWSER,
                        &mut browser,
                    )
                },
                "querying the desktop IShellBrowser",
            )?;
            let browser =
                unsafe { ComPtr::from_raw(browser, "querying the desktop IShellBrowser")? };

            // IShellBrowser::QueryActiveShellView.
            type ViewFn = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
            let active_view: ViewFn = unsafe { mem::transmute(vtable_entry(browser.as_ptr(), 15)) };
            let mut shell_view = ptr::null_mut();
            check(
                unsafe { active_view(browser.as_ptr(), &mut shell_view) },
                "getting the active desktop IShellView",
            )?;
            let shell_view =
                unsafe { ComPtr::from_raw(shell_view, "getting the active desktop IShellView")? };
            // IFolderView2 inherits IFolderView, so all existing IFolderView slots remain valid.
            let view = shell_view.query_interface(&IID_IFOLDER_VIEW2, "querying IFolderView2")?;
            let options = shell_view
                .query_interface(&IID_IFOLDER_VIEW_OPTIONS, "querying IFolderViewOptions")?;
            Ok(Self {
                view,
                options,
                scratch_pidls: RefCell::new(Vec::new()),
                scratch_points: RefCell::new(Vec::new()),
            })
        }

        /// 读取桌面当前的 FOLDERFLAGS，包括自动排列和网格对齐状态。
        fn current_folder_flags(&self) -> Result<u32> {
            // IFolderView2::GetCurrentFolderFlags.
            type Fn = unsafe extern "system" fn(*mut c_void, *mut u32) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.view.as_ptr(), 25)) };
            let mut flags = 0;
            check(
                unsafe { call(self.view.as_ptr(), &mut flags) },
                "reading desktop folder flags",
            )?;
            Ok(flags)
        }

        /// 按掩码修改桌面 FOLDERFLAGS；掩码之外的用户设置保持不变。
        fn set_folder_flags(&self, mask: u32, flags: u32) -> Result<()> {
            // IFolderView2::SetCurrentFolderFlags.
            type Fn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.view.as_ptr(), 24)) };
            check(
                unsafe { call(self.view.as_ptr(), mask, flags) },
                "setting desktop folder flags",
            )
        }

        /// 读取 IFolderViewOptions 当前启用的视图选项。
        fn folder_view_options(&self) -> Result<u32> {
            type Fn = unsafe extern "system" fn(*mut c_void, *mut u32) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.options.as_ptr(), 4)) };
            let mut options = 0;
            check(
                unsafe { call(self.options.as_ptr(), &mut options) },
                "reading desktop folder view options",
            )?;
            Ok(options)
        }

        /// 按掩码修改桌面视图选项，用于临时允许图标使用任意 X/Y 坐标。
        fn set_folder_view_options(&self, mask: u32, options: u32) -> Result<()> {
            type Fn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.options.as_ptr(), 3)) };
            let hr = unsafe { call(self.options.as_ptr(), mask, options) };
            // 部分 Explorer 桌面实现会公开 IFolderViewOptions，但设置方法固定返回
            // E_NOTIMPL。桌面本身仍支持 SelectAndPositionItems，因此可安全跳过。
            if hr == E_NOTIMPL {
                Ok(())
            } else {
                check(hr, "setting desktop folder view options")
            }
        }

        /// 读取 PIDL 的桌面绝对解析名，并通过规范 GUID 判断它是否为系统回收站。
        fn is_recycle_bin(&self, pidl: &Pidl) -> Result<bool> {
            let mut raw_name = ptr::null_mut();
            check(
                unsafe {
                    SHGetNameFromIDList(pidl.as_ptr(), SIGDN_DESKTOPABSOLUTEPARSING, &mut raw_name)
                },
                "reading a desktop item's parsing name",
            )?;
            let raw_name = NonNull::new(raw_name)
                .ok_or_else(|| Error("Shell returned a null parsing name".into()))?;
            let mut length = 0usize;
            unsafe {
                while *raw_name.as_ptr().add(length) != 0 {
                    length += 1;
                }
            }
            let name = String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(raw_name.as_ptr(), length)
            });
            unsafe { CoTaskMemFree(raw_name.as_ptr().cast()) };
            Ok(name.to_ascii_lowercase().contains(RECYCLE_BIN_GUID))
        }

        /// 枚举全部桌面项目 PIDL，读取每个项目的初始坐标并组成图标快照。
        fn icons(&self) -> Result<Vec<Icon>> {
            // IFolderView::Items(SVGIO_ALL, IID_IEnumIDList, ...).
            type ItemsFn = unsafe extern "system" fn(
                *mut c_void,
                u32,
                *const Guid,
                *mut *mut c_void,
            ) -> HResult;
            let items: ItemsFn = unsafe { mem::transmute(vtable_entry(self.view.as_ptr(), 8)) };
            let mut enumerator = ptr::null_mut();
            check(
                unsafe {
                    items(
                        self.view.as_ptr(),
                        SVGIO_ALL,
                        &IID_IENUM_ID_LIST,
                        &mut enumerator,
                    )
                },
                "enumerating desktop items",
            )?;
            let enumerator = unsafe { ComPtr::from_raw(enumerator, "enumerating desktop items")? };

            type NextFn =
                unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> HResult;
            let next: NextFn = unsafe { mem::transmute(vtable_entry(enumerator.as_ptr(), 3)) };
            let mut result = Vec::new();
            loop {
                let mut raw_pidl = ptr::null_mut();
                let mut fetched = 0;
                let hr = unsafe { next(enumerator.as_ptr(), 1, &mut raw_pidl, &mut fetched) };
                if hr == S_FALSE || fetched == 0 {
                    break;
                }
                check(hr, "reading a desktop item")?;
                let pidl =
                    Pidl(NonNull::new(raw_pidl).ok_or_else(|| {
                        Error("desktop item enumerator returned a null PIDL".into())
                    })?);
                let original = self.item_position(&pidl)?;
                let is_recycle_bin = self.is_recycle_bin(&pidl)?;
                result.push(Icon {
                    pidl,
                    original,
                    is_recycle_bin,
                });
            }
            Ok(result)
        }

        /// 通过项目 PIDL 查询单个桌面图标当前的屏幕坐标。
        fn item_position(&self, pidl: &Pidl) -> Result<Point> {
            type Fn = unsafe extern "system" fn(*mut c_void, *const c_void, *mut Point) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.view.as_ptr(), 11)) };
            let mut point = Point { x: 0, y: 0 };
            check(
                unsafe { call(self.view.as_ptr(), pidl.as_ptr(), &mut point) },
                "reading a desktop icon position",
            )?;
            Ok(point)
        }

        /// 将 PIDL 和目标坐标组成等长数组，通过一次 Shell 调用批量更新所有图标位置。
        fn position_all(&self, icons: &[Icon], points: &[Point]) -> Result<()> {
            if icons.len() != points.len() {
                return Err(Error("internal PIDL/position count mismatch".into()));
            }
            let mut pidls = self.scratch_pidls.borrow_mut();
            pidls.clear();
            pidls.extend(icons.iter().map(|icon| icon.pidl.as_ptr()));
            self.position_raw(&pidls, points, "updating desktop icon positions")
        }

        /// 只向 Shell 提交指定索引对应的物理刚体坐标，并复用内部临时缓冲区。
        fn position_body_indices(
            &self,
            icons: &[Icon],
            bodies: &[Body],
            indices: &[usize],
        ) -> Result<()> {
            if icons.len() != bodies.len() {
                return Err(Error("internal icon/body count mismatch".into()));
            }
            if indices.is_empty() {
                return Ok(());
            }
            let mut pidls = self.scratch_pidls.borrow_mut();
            let mut selected_points = self.scratch_points.borrow_mut();
            pidls.clear();
            selected_points.clear();
            for &index in indices {
                let icon = icons
                    .get(index)
                    .ok_or_else(|| Error("moving icon index is out of range".into()))?;
                let body = bodies
                    .get(index)
                    .ok_or_else(|| Error("moving body index is out of range".into()))?;
                pidls.push(icon.pidl.as_ptr());
                selected_points.push(Point {
                    x: body.x.round() as i32,
                    y: body.y.round() as i32,
                });
            }
            self.position_raw(
                &pidls,
                &selected_points,
                "updating moving desktop icon positions",
            )
        }

        /// 扫描物理状态并只提交已经释放的图标，避免为索引和坐标创建逐帧 Vec。
        fn position_released_bodies(&self, icons: &[Icon], bodies: &[Body]) -> Result<()> {
            if icons.len() != bodies.len() {
                return Err(Error("internal icon/body count mismatch".into()));
            }
            let mut pidls = self.scratch_pidls.borrow_mut();
            let mut points = self.scratch_points.borrow_mut();
            pidls.clear();
            points.clear();
            for (icon, body) in icons.iter().zip(bodies).filter(|(_, body)| body.released) {
                pidls.push(icon.pidl.as_ptr());
                points.push(Point {
                    x: body.x.round() as i32,
                    y: body.y.round() as i32,
                });
            }
            self.position_raw(&pidls, &points, "updating released desktop icon positions")
        }

        /// 调用 IFolderView::SelectAndPositionItems 提交已经准备好的 PIDL 与坐标数组。
        fn position_raw(
            &self,
            pidls: &[*const c_void],
            points: &[Point],
            operation: &str,
        ) -> Result<()> {
            if pidls.len() != points.len() {
                return Err(Error("internal raw PIDL/position count mismatch".into()));
            }
            if pidls.is_empty() {
                return Ok(());
            }
            let count = u32::try_from(pidls.len())
                .map_err(|_| Error("too many desktop icons to update".into()))?;
            type Fn = unsafe extern "system" fn(
                *mut c_void,
                u32,
                *const *const c_void,
                *const Point,
                u32,
            ) -> HResult;
            let call: Fn = unsafe { mem::transmute(vtable_entry(self.view.as_ptr(), 16)) };
            check(
                unsafe {
                    call(
                        self.view.as_ptr(),
                        count,
                        pidls.as_ptr(),
                        points.as_ptr(),
                        SVSI_POSITIONITEM,
                    )
                },
                operation,
            )
        }
    }

    struct LayoutGuard {
        view: FolderView,
        icons: Vec<Icon>,
        original_folder_flags: u32,
        original_view_options: u32,
        restored: bool,
    }

    impl LayoutGuard {
        /// 保存原始排列设置，启用自定义坐标，并临时关闭自动排列与网格对齐。
        ///
        /// 守卫在修改设置之前就已创建，因此后续任一步失败也会触发自动恢复。
        fn prepare(view: FolderView, icons: Vec<Icon>) -> Result<Self> {
            let original_folder_flags = view.current_folder_flags()?;
            let original_view_options = view.folder_view_options()?;
            let layout = Self {
                view,
                icons,
                original_folder_flags,
                original_view_options,
                restored: false,
            };

            // Windows 7+ requires FVO_CUSTOMPOSITION when applying folder flags that
            // permit arbitrary X/Y positions. The Drop guard is already live here.
            RESTORED.store(false, Ordering::Release);
            layout.set_custom_positioning(true)?;
            layout.view.set_folder_flags(ARRANGEMENT_FLAGS, 0)?;
            Ok(layout)
        }

        /// 设置或清除 FVO_CUSTOMPOSITION，使 Shell 接受任意图标坐标。
        fn set_custom_positioning(&self, enabled: bool) -> Result<()> {
            let value = if enabled { FVO_CUSTOMPOSITION } else { 0 };
            self.view.set_folder_view_options(FVO_CUSTOMPOSITION, value)
        }

        /// 恢复全部原始坐标、排列标志和视图选项；即使某一步失败也继续尝试其余步骤。
        fn restore(&mut self) -> Result<()> {
            if self.restored {
                return Ok(());
            }
            let mut first_error = None;
            let points: Vec<Point> = self.icons.iter().map(|icon| icon.original).collect();
            if let Err(error) = self.view.position_all(&self.icons, &points) {
                first_error = Some(error);
            }
            if let Err(error) = self.view.set_folder_flags(
                ARRANGEMENT_FLAGS,
                self.original_folder_flags & ARRANGEMENT_FLAGS,
            ) {
                first_error.get_or_insert(error);
            }
            let custom_position_was_enabled = self.original_view_options & FVO_CUSTOMPOSITION != 0;
            if let Err(error) = self.set_custom_positioning(custom_position_was_enabled) {
                first_error.get_or_insert(error);
            }
            self.restored = true;
            RESTORED.store(true, Ordering::Release);
            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    impl Drop for LayoutGuard {
        /// 在正常退出、错误返回或 panic 展开时执行最后一道布局恢复保护。
        fn drop(&mut self) {
            if let Err(error) = self.restore() {
                eprintln!("warning: could not restore the original desktop layout: {error}");
            }
        }
    }

    struct ConsoleHandler;

    impl ConsoleHandler {
        /// 注册 Windows 控制台事件处理器，并初始化动画运行与恢复状态。
        fn install() -> Result<Self> {
            RUNNING.store(true, Ordering::Release);
            // No mutable desktop state exists until LayoutGuard::prepare starts.
            RESTORED.store(true, Ordering::Release);
            if unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 1) } == 0 {
                Err(Error(
                    "installing the console control handler failed".into(),
                ))
            } else {
                Ok(Self)
            }
        }
    }

    impl Drop for ConsoleHandler {
        /// 程序离开运行作用域时注销控制台事件处理器，避免保留失效回调。
        fn drop(&mut self) {
            unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 0) };
        }
    }

    struct HotkeyHandler;

    impl HotkeyHandler {
        /// 在独立消息线程注册全局 Ctrl+Alt+Q；触发后通知主线程正常停止动画。
        fn install() -> Result<Self> {
            let (sender, receiver) = mpsc::sync_channel(1);
            thread::Builder::new()
                .name("exit-hotkey".into())
                .spawn(move || {
                    let registered = unsafe {
                        RegisterHotKey(0, HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_Q)
                    } != 0;
                    let _ = sender.send(registered);
                    if !registered {
                        return;
                    }

                    let mut message: Message = unsafe { mem::zeroed() };
                    loop {
                        let status = unsafe { GetMessageW(&mut message, 0, 0, 0) };
                        if status <= 0 {
                            break;
                        }
                        if message.message == WM_HOTKEY && message.w_param == HOTKEY_ID as usize {
                            RUNNING.store(false, Ordering::Release);
                            break;
                        }
                    }
                    unsafe { UnregisterHotKey(0, HOTKEY_ID) };
                })
                .map_err(|error| Error(format!("starting the hotkey thread failed: {error}")))?;

            match receiver.recv() {
                Ok(true) => Ok(Self),
                Ok(false) => Err(Error(
                    "registering Ctrl+Alt+Q failed; the shortcut may already be in use".into(),
                )),
                Err(error) => Err(Error(format!(
                    "the hotkey thread stopped during startup: {error}"
                ))),
            }
        }
    }

    /// 处理 Ctrl+C、控制台关闭、注销和关机事件，通知主线程停止并等待布局恢复。
    unsafe extern "system" fn console_control_handler(event: u32) -> i32 {
        match event {
            0 | 1 => {
                RUNNING.store(false, Ordering::Release);
                1
            }
            2 | 5 | 6 => {
                RUNNING.store(false, Ordering::Release);
                // Close/logoff/shutdown may terminate the process as soon as this callback returns.
                // Give the COM-owning main thread a bounded window in which to restore the layout.
                for _ in 0..400 {
                    if RESTORED.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                1
            }
            _ => 0,
        }
    }

    /// 从 COM 对象的虚函数表中读取指定槽位的方法地址。
    ///
    /// 调用方必须保证对象非空、接口类型正确且索引属于该接口的有效虚函数槽位。
    unsafe fn vtable_entry(object: *mut c_void, index: usize) -> ComMethod {
        unsafe {
            let table = *(object as *const *const ComMethod);
            *table.add(index)
        }
    }

    /// 查询指定桌面坐标所在显示器的工作区，避免图标落到任务栏后面。
    fn monitor_work_area(point: Point) -> Result<Rect> {
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
        if monitor == 0 {
            return Err(Error(
                "finding the monitor for a desktop icon failed".into(),
            ));
        }
        let mut info = MonitorInfo {
            size: u32::try_from(mem::size_of::<MonitorInfo>())
                .map_err(|_| Error("invalid MONITORINFO size".into()))?,
            monitor: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            work: Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            flags: 0,
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return Err(Error("reading the monitor work area failed".into()));
        }
        Ok(info.work)
    }

    /// 判断两个工作区矩形是否属于同一个显示器区域。
    fn same_rect(first: Rect, second: Rect) -> bool {
        first.left == second.left
            && first.top == second.top
            && first.right == second.right
            && first.bottom == second.bottom
    }

    /// 根据显示器为普通图标计算顶部槽位，并为回收站计算底部中央的接收位置。
    fn create_bodies(icons: &[Icon]) -> Result<Vec<Body>> {
        let bounds: Vec<Rect> = icons
            .iter()
            .map(|icon| monitor_work_area(icon.original))
            .collect::<Result<_>>()?;
        let mut top_positions = vec![Point { x: 0, y: 0 }; icons.len()];
        for index in 0..icons.len() {
            let mut monitor_icons: Vec<usize> = (0..icons.len())
                .filter(|&other| same_rect(bounds[index], bounds[other]))
                .collect();
            monitor_icons.sort_by_key(|&other| {
                let point = icons[other].original;
                (point.x, point.y)
            });
            let rank = monitor_icons
                .iter()
                .position(|&other| other == index)
                .ok_or_else(|| Error("building the top icon layout failed".into()))?;
            let usable_width = (bounds[index].right - bounds[index].left - 24).max(76);
            let columns = usize::try_from(usable_width / 76)
                .map_err(|_| Error("invalid monitor work-area width".into()))?
                .max(1);
            top_positions[index] = Point {
                x: bounds[index].left + 12 + (rank % columns) as i32 * 76,
                y: bounds[index].top + 12 + (rank / columns) as i32 * 76,
            };
        }

        icons
            .iter()
            .enumerate()
            .map(|(index, icon)| {
                let point = icon.original;
                Ok(Body {
                    x: f64::from(point.x),
                    y: f64::from(point.y),
                    top_x: f64::from(top_positions[index].x),
                    top_y: f64::from(top_positions[index].y),
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    release_at: 0.0,
                    released: false,
                    bounds: bounds[index],
                })
            })
            .collect()
    }

    /// 将单个刚体限制在显示器工作区内，并对边界碰撞施加弹性和地面摩擦。
    fn resolve_screen_collision(body: &mut Body) {
        let left = f64::from(body.bounds.left);
        let right = f64::from(body.bounds.right) - ICON_DIAMETER;
        let top = f64::from(body.bounds.top);
        // MONITORINFO.rcWork 已经排除了任务栏；再减去图标高度后，
        // 图标底边会正好停在当前显示器任务栏的顶部。
        let bottom = f64::from(body.bounds.bottom) - ICON_DIAMETER;

        if body.x < left {
            body.x = left;
            body.velocity_x = body.velocity_x.abs() * 0.55;
        } else if body.x > right {
            body.x = right;
            body.velocity_x = -body.velocity_x.abs() * 0.55;
        }
        if body.y < top {
            body.y = top;
            body.velocity_y = body.velocity_y.abs() * 0.5;
        } else if body.y > bottom {
            body.y = bottom;
            if body.velocity_y > 24.0 {
                body.velocity_y = -body.velocity_y * 0.48;
            } else {
                body.velocity_y = 0.0;
            }
            body.velocity_x *= 0.88;
            if body.velocity_x.abs() < 2.0 {
                body.velocity_x = 0.0;
            }
        }
    }

    /// 检测两个已释放图标的圆形碰撞，修正重叠并按等质量弹性碰撞计算速度。
    fn resolve_body_pair(first: &mut Body, second: &mut Body) {
        if !first.released || !second.released {
            return;
        }
        let delta_x = second.x - first.x;
        let delta_y = second.y - first.y;
        let distance_squared = delta_x * delta_x + delta_y * delta_y;
        if distance_squared >= ICON_DIAMETER * ICON_DIAMETER {
            return;
        }

        let (normal_x, normal_y, distance) = if distance_squared > 0.0001 {
            let distance = distance_squared.sqrt();
            (delta_x / distance, delta_y / distance, distance)
        } else {
            (1.0, 0.0, 0.0)
        };
        let correction = (ICON_DIAMETER - distance) * 0.5;
        first.x -= normal_x * correction;
        first.y -= normal_y * correction;
        second.x += normal_x * correction;
        second.y += normal_y * correction;

        let relative_velocity = (second.velocity_x - first.velocity_x) * normal_x
            + (second.velocity_y - first.velocity_y) * normal_y;
        if relative_velocity < 0.0 {
            let impulse = -(1.0 + 0.42) * relative_velocity * 0.5;
            first.velocity_x -= impulse * normal_x;
            first.velocity_y -= impulse * normal_y;
            second.velocity_x += impulse * normal_x;
            second.velocity_y += impulse * normal_y;
        }
    }

    /// 使用固定上限的小时间步推进重力、图标间碰撞和屏幕边界碰撞。
    fn step_physics(bodies: &mut [Body], elapsed: f64, delta: f64) {
        let steps = (delta / 0.008).ceil().max(1.0) as usize;
        let step = delta / steps as f64;
        for _ in 0..steps {
            for body in bodies.iter_mut() {
                if !body.released && elapsed >= body.release_at {
                    body.released = true;
                }
                if body.released {
                    body.velocity_y += GRAVITY * step;
                    body.x += body.velocity_x * step;
                    body.y += body.velocity_y * step;
                    resolve_screen_collision(body);
                }
            }

            for first_index in 0..bodies.len() {
                for second_index in first_index + 1..bodies.len() {
                    let (left, right) = bodies.split_at_mut(second_index);
                    resolve_body_pair(&mut left[first_index], &mut right[0]);
                }
            }
            for body in bodies.iter_mut().filter(|body| body.released) {
                resolve_screen_collision(body);
            }
        }
    }

    /// 将浮点物理坐标舍入成 Windows Shell 接口需要的整数 POINT 数组。
    fn body_points(bodies: &[Body]) -> Vec<Point> {
        bodies
            .iter()
            .map(|body| Point {
                x: body.x.round() as i32,
                y: body.y.round() as i32,
            })
            .collect()
    }

    /// 判断所有已释放图标是否基本停止，用于提前结束物理模拟阶段。
    fn bodies_are_stable(bodies: &[Body]) -> bool {
        bodies.iter().all(|body| {
            body.released && body.velocity_x.abs() < 18.0 && body.velocity_y.abs() < 24.0
        })
    }

    /// 使用基于当前时间的 XorShift 随机源原地打乱图标索引。
    fn shuffle_indices(indices: &mut [usize]) {
        let mut state = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ (indices.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for index in (1..indices.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            indices.swap(index, state as usize % (index + 1));
        }
    }

    /// 随机生成一轮重力下落的释放顺序，并重置所有图标的初速度和运行状态。
    fn reset_for_rain(bodies: &mut [Body]) {
        let mut release_order: Vec<usize> = (0..bodies.len()).collect();
        shuffle_indices(&mut release_order);
        for (order, &index) in release_order.iter().enumerate() {
            let body = &mut bodies[index];
            body.x = body.top_x;
            body.y = body.top_y;
            body.velocity_x = ((index * 37 % 11) as f64 - 5.0) * 7.0;
            body.velocity_y = 0.0;
            body.release_at = order as f64 * 0.45;
            body.released = false;
        }
    }

    /// 首次启动时将所有图标同时平滑移动到顶部槽位。
    fn arrange_at_top(layout: &LayoutGuard, bodies: &mut [Body]) -> Result<()> {
        let starts = body_points(bodies);
        let duration = 0.9;
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let progress = (elapsed / duration).min(1.0);
            let smooth = progress * progress * (3.0 - 2.0 * progress);
            for (index, body) in bodies.iter_mut().enumerate() {
                body.x =
                    f64::from(starts[index].x) + (body.top_x - f64::from(starts[index].x)) * smooth;
                body.y =
                    f64::from(starts[index].y) + (body.top_y - f64::from(starts[index].y)) * smooth;
            }
            let points = body_points(bodies);
            layout.view.position_all(&layout.icons, &points)?;
            if progress >= 1.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        reset_bodies_at_top(bodies);
        Ok(())
    }

    /// 将所有物理刚体精确重置到顶部槽位，并清除上一轮速度。
    fn reset_bodies_at_top(bodies: &mut [Body]) {
        for body in bodies {
            body.x = body.top_x;
            body.y = body.top_y;
            body.velocity_x = 0.0;
            body.velocity_y = 0.0;
            body.released = false;
        }
    }

    /// 每吞入一个图标后让回收站下沉、上弹并抖动，近似表现缩放和吞咽反馈。
    fn recycle_bin_reaction(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
    ) -> Result<()> {
        let base_x = bodies[recycle_index].x;
        let base_y = bodies[recycle_index].y;
        let duration = 0.48;
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let progress = (start.elapsed().as_secs_f64() / duration).min(1.0);
            let decay = 1.0 - progress;
            let offset_x = (progress * std::f64::consts::TAU * 7.0).sin() * 6.0 * decay;
            let offset_y = if progress < 0.35 {
                progress / 0.35 * 7.0
            } else {
                -(progress * std::f64::consts::PI).sin() * 11.0
            };
            bodies[recycle_index].x = base_x + offset_x;
            bodies[recycle_index].y = base_y + offset_y;
            render_moving_indices(layout, bodies, &[recycle_index])?;
            if progress >= 1.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        bodies[recycle_index].x = base_x;
        bodies[recycle_index].y = base_y;
        Ok(())
    }

    /// 最后一个图标被吞入后，让回收站以逐渐增大的幅度抖动并为爆炸蓄力。
    fn charge_recycle_bin_explosion(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
    ) -> Result<()> {
        let base_x = bodies[recycle_index].x;
        let base_y = bodies[recycle_index].y;
        let duration = 0.8;
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let progress = (start.elapsed().as_secs_f64() / duration).min(1.0);
            let amplitude = 2.0 + progress * 12.0;
            let offset_x = (progress * std::f64::consts::TAU * 13.0).sin() * amplitude;
            let offset_y = (progress * std::f64::consts::TAU * 17.0).cos() * amplitude * 0.45;
            bodies[recycle_index].x = base_x + offset_x;
            bodies[recycle_index].y = base_y + offset_y;
            render_moving_indices(layout, bodies, &[recycle_index])?;
            if progress >= 1.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        Ok(())
    }

    /// 从回收站位置同时炸出所有普通图标，越过顶部槽位后回弹并精确归位。
    fn explode_from_recycle_bin(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
        consumed: &[usize],
    ) -> Result<()> {
        let center_x = bodies[recycle_index].x;
        let center_y = bodies[recycle_index].y;
        let duration = 1.2;
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let progress = (start.elapsed().as_secs_f64() / duration).min(1.0);
            for &index in consumed {
                let delta_x = bodies[index].top_x - center_x;
                let delta_y = bodies[index].top_y - center_y;
                let distance = delta_x.hypot(delta_y).max(1.0);
                let overshoot_x = bodies[index].top_x + delta_x / distance * 55.0;
                let overshoot_y = bodies[index].top_y + delta_y / distance * 55.0;
                if progress < 0.42 {
                    let local = progress / 0.42;
                    let ease_out = 1.0 - (1.0 - local).powi(3);
                    bodies[index].x = center_x + (overshoot_x - center_x) * ease_out;
                    bodies[index].y = center_y + (overshoot_y - center_y) * ease_out;
                } else {
                    let local = (progress - 0.42) / 0.58;
                    let smooth = local * local * (3.0 - 2.0 * local);
                    bodies[index].x = overshoot_x + (bodies[index].top_x - overshoot_x) * smooth;
                    bodies[index].y = overshoot_y + (bodies[index].top_y - overshoot_y) * smooth;
                }
            }
            bodies[recycle_index].x = center_x;
            bodies[recycle_index].y = center_y;
            render_catch_scene(layout, bodies, recycle_index)?;
            if progress >= 1.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        reset_bodies_at_top(bodies);
        Ok(())
    }

    /// 批量写入场景坐标后再次单独写入回收站，尽量让重合的已回收图标被它遮住。
    fn render_catch_scene(
        layout: &LayoutGuard,
        bodies: &[Body],
        recycle_index: usize,
    ) -> Result<()> {
        let points = body_points(bodies);
        layout.view.position_all(&layout.icons, &points)?;
        layout.view.position_all(
            std::slice::from_ref(&layout.icons[recycle_index]),
            std::slice::from_ref(&points[recycle_index]),
        )
    }

    /// 只刷新当前正在运动的图标集合，避免干扰顶部静止图标的点击和选择状态。
    fn render_moving_indices(
        layout: &LayoutGuard,
        bodies: &[Body],
        indices: &[usize],
    ) -> Result<()> {
        layout
            .view
            .position_body_indices(&layout.icons, bodies, indices)
    }

    /// 使用轻量 XorShift 随机打乱普通图标顺序，使每轮垂直掉落的选择不同。
    fn randomized_drop_order(icon_count: usize, recycle_index: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..icon_count)
            .filter(|&index| index != recycle_index)
            .collect();
        shuffle_indices(&mut order);
        order
    }

    /// 让选中图标先垂直下落，短暂延迟后回收站才移动到落点完成接球。
    fn drop_first_then_catch(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
        active_index: usize,
    ) -> Result<()> {
        const TOTAL_DURATION: f64 = 1.0;
        const RECYCLE_START: f64 = 0.2;
        const RECYCLE_TRAVEL: f64 = 0.58;

        let fixed_x = bodies[active_index].x;
        let icon_start_y = bodies[active_index].y;
        let catch_y = f64::from(bodies[active_index].bounds.bottom) - ICON_DIAMETER - 18.0;
        let recycle_start_x = bodies[recycle_index].x;
        let recycle_start_y = bodies[recycle_index].y;
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let fall_progress = (elapsed / TOTAL_DURATION).min(1.0);
            bodies[active_index].x = fixed_x;
            bodies[active_index].y =
                icon_start_y + (catch_y - icon_start_y) * fall_progress * fall_progress;

            let catch_progress = ((elapsed - RECYCLE_START) / RECYCLE_TRAVEL).clamp(0.0, 1.0);
            let smooth = catch_progress * catch_progress * (3.0 - 2.0 * catch_progress);
            bodies[recycle_index].x = recycle_start_x + (fixed_x - recycle_start_x) * smooth;
            bodies[recycle_index].y = recycle_start_y + (catch_y - recycle_start_y) * smooth;
            render_moving_indices(layout, bodies, &[active_index, recycle_index])?;
            if fall_progress >= 1.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        // 接住后的最后一帧直接跳到屏幕外，不对这次坐标变化做插值动画。
        bodies[active_index].x = f64::from(bodies[active_index].bounds.right) + 2048.0;
        bodies[active_index].y = f64::from(bodies[active_index].bounds.bottom) + 2048.0;
        render_moving_indices(layout, bodies, &[active_index, recycle_index])?;
        Ok(())
    }

    /// 执行单轮渐进释放、重力下落、碰撞和弹跳，稳定或超时后返回。
    fn run_rain_cycle(layout: &LayoutGuard, bodies: &mut [Body]) -> Result<()> {
        reset_for_rain(bodies);
        let release_finished_at = bodies
            .iter()
            .map(|body| body.release_at)
            .fold(0.0, f64::max);
        let start = Instant::now();
        let mut previous = start;
        let mut stable_for = 0.0;
        while RUNNING.load(Ordering::Acquire) {
            let frame_start = Instant::now();
            let elapsed = start.elapsed().as_secs_f64();
            let delta = frame_start
                .duration_since(previous)
                .as_secs_f64()
                .min(0.033);
            previous = frame_start;
            step_physics(bodies, elapsed, delta);
            layout
                .view
                .position_released_bodies(&layout.icons, bodies)?;

            if elapsed >= release_finished_at && bodies_are_stable(bodies) {
                stable_for += delta;
            } else {
                stable_for = 0.0;
            }
            if stable_for >= 0.8 || elapsed >= release_finished_at + 7.0 {
                break;
            }
            thread::sleep(FRAME_TIME.saturating_sub(frame_start.elapsed()));
        }
        Ok(())
    }

    /// 在保持当前图标位置不变的情况下等待指定时长，并允许退出快捷键随时打断。
    fn wait_interruptibly(duration: Duration) {
        let start = Instant::now();
        while RUNNING.load(Ordering::Acquire) && start.elapsed() < duration {
            thread::sleep(FRAME_TIME.min(duration.saturating_sub(start.elapsed())));
        }
    }

    /// 将回收站从顶部槽位移动到底部中央，准备执行移动接球模式。
    fn place_recycle_bin_at_bottom(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
    ) -> Result<()> {
        let bounds = bodies[recycle_index].bounds;
        bodies[recycle_index].x =
            f64::from(bounds.left + (bounds.right - bounds.left) / 2) - ICON_DIAMETER * 0.5;
        bodies[recycle_index].y = f64::from(bounds.bottom) - ICON_DIAMETER - 18.0;
        render_moving_indices(layout, bodies, &[recycle_index])
    }

    /// 完整执行一轮回收站接球模式，爆炸结束后所有图标回到顶部槽位。
    fn run_recycle_bin_catch_mode(
        layout: &LayoutGuard,
        bodies: &mut [Body],
        recycle_index: usize,
    ) -> Result<()> {
        place_recycle_bin_at_bottom(layout, bodies, recycle_index)?;
        let drop_order = randomized_drop_order(layout.icons.len(), recycle_index);
        let mut consumed = Vec::with_capacity(drop_order.len());
        wait_interruptibly(Duration::from_millis(650));
        for &index in &drop_order {
            if !RUNNING.load(Ordering::Acquire) {
                return Ok(());
            }
            drop_first_then_catch(layout, bodies, recycle_index, index)?;
            consumed.push(index);
            recycle_bin_reaction(layout, bodies, recycle_index)?;
            wait_interruptibly(Duration::from_millis(100));
        }
        if RUNNING.load(Ordering::Acquire) {
            charge_recycle_bin_explosion(layout, bodies, recycle_index)?;
            explode_from_recycle_bin(layout, bodies, recycle_index, &consumed)?;
        }
        Ok(())
    }

    /// 完整执行一轮重力碰撞模式，图标稳定后停留并重新排列到顶部。
    fn run_gravity_bounce_mode(layout: &LayoutGuard, bodies: &mut [Body]) -> Result<()> {
        run_rain_cycle(layout, bodies)?;
        if RUNNING.load(Ordering::Acquire) {
            wait_interruptibly(Duration::from_secs(2));
            arrange_at_top(layout, bodies)?;
            wait_interruptibly(Duration::from_millis(650));
        }
        Ok(())
    }

    /// 使用模式状态机在回收站接球与重力碰撞两种动画之间循环切换。
    fn animate(mut layout: LayoutGuard) -> Result<()> {
        let mut bodies = create_bodies(&layout.icons)?;
        let recycle_index = layout.icons.iter().position(|icon| icon.is_recycle_bin);
        if recycle_index.is_some() {
            println!(
                "开始交替播放 {}图标动画 按 Ctrl+Alt+Q 停止.",
                layout.icons.len() - 1
            );
        } else {
            println!(
                "开始交替播放 {} 图标动画 按 Ctrl+Alt+Q 停止.",
                layout.icons.len()
            );
        }
        arrange_at_top(&layout, &mut bodies)?;
        let mut mode = AnimationMode::GravityBounce;

        while RUNNING.load(Ordering::Acquire) {
            match mode {
                AnimationMode::RecycleBinCatch => run_recycle_bin_catch_mode(
                    &layout,
                    &mut bodies,
                    recycle_index.ok_or_else(|| {
                        Error("internal animation mode requires a Recycle Bin".into())
                    })?,
                )?,
                AnimationMode::GravityBounce => run_gravity_bounce_mode(&layout, &mut bodies)?,
            }
            mode = if recycle_index.is_some() {
                mode.next()
            } else {
                AnimationMode::GravityBounce
            };
        }
        println!("Restoring the original desktop layout...");
        layout.restore()
    }

    /// 执行应用主流程：初始化 COM、读取桌面、处理只读检查或进入动画循环。
    pub fn main() -> Result<()> {
        let check_only = std::env::args_os().any(|argument| argument == "--check");
        let _com = ComApartment::init()?;
        let view = FolderView::desktop()?;
        let icons = view.icons()?;
        if icons.is_empty() {
            return Err(Error("no desktop icons were found".into()));
        }
        let recycle_count = icons.iter().filter(|icon| icon.is_recycle_bin).count();
        if recycle_count > 1 {
            return Err(Error(format!(
                "expected at most one Recycle Bin icon, found {recycle_count}"
            )));
        }
        println!("保存了 {} 个原始图标位置.", icons.len());
        if check_only {
            let folder_flags = view.current_folder_flags()?;
            let view_options = view.folder_view_options()?;
            let recycle_status = if recycle_count == 1 {
                "found"
            } else {
                "not found"
            };
            println!(
                "Desktop Shell check succeeded (Recycle Bin: {}, auto-arrange: {}, align-to-grid: {}, custom-position: {}); no icons were moved.",
                recycle_status,
                folder_flags & FWF_AUTOARRANGE != 0,
                folder_flags & FWF_SNAPTOGRID != 0,
                view_options & FVO_CUSTOMPOSITION != 0,
            );
            return Ok(());
        }
        let _handler = ConsoleHandler::install()?;
        let _hotkey = HotkeyHandler::install()?;
        let layout = LayoutGuard::prepare(view, icons)?;
        println!("暂时禁用自动排列和对齐网格.");
        animate(layout)
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        /// 初始化当前线程的 COM 单元。
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> HResult;
        /// 反初始化当前线程的 COM 单元。
        fn CoUninitialize();
        /// 按 CLSID 创建 COM 对象并请求指定接口。
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            context: u32,
            iid: *const Guid,
            object: *mut *mut c_void,
        ) -> HResult;
        /// 释放由 COM 任务分配器创建的内存块。
        fn CoTaskMemFree(memory: *const c_void);
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        /// 注册或注销当前进程的控制台控制事件回调。
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        /// 为当前线程注册一个系统级快捷键。
        fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, virtual_key: u32) -> i32;
        /// 注销当前线程注册的系统级快捷键。
        fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
        /// 从当前线程消息队列获取下一条消息。
        fn GetMessageW(message: *mut Message, hwnd: isize, min: u32, max: u32) -> i32;
        /// 返回包含指定点的显示器；点不在显示器内时选择距离最近的显示器。
        fn MonitorFromPoint(point: Point, flags: u32) -> isize;
        /// 读取显示器边界和排除任务栏后的工作区边界。
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        /// 获取 PIDL 的规范 Shell 解析名，返回的字符串由调用方使用 CoTaskMemFree 释放。
        fn SHGetNameFromIDList(pidl: *const c_void, form: u32, name: *mut *mut u16) -> HResult;
    }
}

#[cfg(all(windows, target_pointer_width = "64"))]
/// 进程入口：运行应用并将顶层错误输出到标准错误流。
fn main() {
    if let Err(error) = app::main() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
