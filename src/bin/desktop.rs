//! 桌面版：在自己的窗口（WebView2）里打开 AI 小说工坊，不用浏览器，也没有命令行窗口。
//! 数据默认放在 exe 旁边的 data 目录（和浏览器版共用）；那里不可写时放到 %APPDATA%\ai-novel\data，
//! 也可以用环境变量 AI_NOVEL_DATA 指定。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_novel::{build_router, db::Db, state::AppState};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use std::time::Duration;
use tao::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
use tao::window::{Icon, Window, WindowBuilder};
use tracing_subscriber::EnvFilter;
use wry::{NewWindowResponse, WebContext, WebViewBuilder};

const TITLE: &str = "AI 小说工坊";
/// 固定端口：页面里的偏好（主题、面板、灰字设置等）按地址保存，端口变了就会丢
const PORT: u16 = 18686;
/// `--serve` 后台服务的端口，和窗口版、浏览器版（8686）错开，可以同时开
const SERVE_PORT: u16 = 18688;
const ALREADY_RUNNING: &str = "AI 小说工坊已经打开了，请在任务栏里找到它。";

enum UserEvent {
    /// 页面已经保存完正在编辑的内容，可以关窗口了
    Close,
    /// 导出的文件下载完成
    Downloaded(PathBuf),
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("AI_NOVEL_DATA").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join("data")));
    if let Some(d) = beside.filter(|d| writable(d)) {
        return d;
    }
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("ai-novel").join("data")
}

fn init_log(dir: &Path) {
    let path = dir.join("ai-novel.log");
    let too_big = std::fs::metadata(&path).map(|m| m.len() > 5 * 1024 * 1024).unwrap_or(false);
    let file = std::fs::OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(&path);
    if let Ok(file) = file {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
            .try_init();
    }
}

/// 端口上已经是我们自己的程序
fn already_running(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
    if s.write_all(b"GET /api/health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.contains("\"name\":\"ai-novel\"")
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn message(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide(TITLE).as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

#[cfg(not(windows))]
fn message(text: &str) {
    eprintln!("{text}");
}

/// 用系统默认程序打开网址
#[cfg(windows)]
fn open_external(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe { ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), wide(target).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

#[cfg(not(windows))]
fn open_external(target: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(target).spawn();
}

fn start_server(db: Db) -> Result<u16, String> {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(_) if already_running(PORT) => return Err(ALREADY_RUNNING.into()),
        Err(_) => TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("无法启动本地服务：{e}"))?,
    };
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let state = AppState::new(db, None);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("启动异步运行时");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("监听端口");
            if let Err(e) = axum::serve(listener, build_router(state)).await {
                tracing::error!("本地服务退出：{e}");
            }
        });
    });
    Ok(port)
}

/// 窗口大小和位置（物理像素），关闭时保存，下次按原样打开
#[derive(Serialize, Deserialize, Clone, Copy)]
struct Geometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

fn load_geometry(path: &Path) -> Option<Geometry> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// 上次的位置还在某个显示器里就沿用；否则大屏居中、小屏最大化。上次关窗口时是最大化的，这次也最大化
fn place_window(builder: WindowBuilder, event_loop: &EventLoop<UserEvent>, saved: Option<Geometry>) -> WindowBuilder {
    let builder = builder.with_maximized(saved.is_some_and(|g| g.maximized));
    if let Some(g) = saved.filter(|g| g.width >= 640 && g.height >= 420) {
        let visible = event_loop.available_monitors().any(|m| {
            let (p, s) = (m.position(), m.size());
            g.x + 120 < p.x + s.width as i32 && g.x + g.width as i32 > p.x + 120 && g.y + 40 < p.y + s.height as i32 && g.y >= p.y - 20
        });
        if visible {
            return builder.with_inner_size(PhysicalSize::new(g.width, g.height)).with_position(PhysicalPosition::new(g.x, g.y));
        }
    }
    let Some(m) = event_loop.primary_monitor() else {
        return builder.with_inner_size(LogicalSize::new(1280.0, 820.0));
    };
    let scale = m.scale_factor();
    let screen = m.size().to_logical::<f64>(scale);
    if screen.width < 1400.0 || screen.height < 860.0 {
        return builder.with_inner_size(LogicalSize::new(screen.width * 0.9, screen.height * 0.85)).with_maximized(true);
    }
    let size = LogicalSize::new(1440.0, 920.0);
    let origin = m.position().to_logical::<f64>(scale);
    builder.with_inner_size(size).with_position(LogicalPosition::new(origin.x + (screen.width - size.width) / 2.0, origin.y + (screen.height - size.height) / 2.0))
}

/// 普通状态（没最大化、没最小化）下的位置和大小。最大化时拿不到还原后的尺寸，所以平时就记下来
fn normal_geometry(window: &Window) -> Option<Geometry> {
    if window.is_maximized() || window.is_minimized() {
        return None;
    }
    let pos = window.outer_position().ok().filter(|p| p.x > -10000)?;
    let size = window.inner_size();
    Some(Geometry { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: false })
}

fn save_geometry(path: &Path, window: &Window, normal: Option<Geometry>, was_maximized: bool) {
    // 最小化后 tao 会清掉“最大化”标记，用最小化之前的状态
    let maximized = if window.is_minimized() { was_maximized } else { window.is_maximized() };
    let g = match normal_geometry(window).or(normal) {
        Some(n) => Geometry { maximized, ..n },
        // 一直是最大化、没有普通状态的尺寸：只记最大化，还原尺寸用默认的
        None if maximized => Geometry { x: 0, y: 0, width: 0, height: 0, maximized: true },
        None => return,
    };
    if let Ok(json) = serde_json::to_vec(&g) {
        let _ = std::fs::write(path, json);
    }
}

fn run_window(port: u16, dir: &Path) -> Result<(), String> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    #[cfg(windows)]
    let icon = {
        use tao::platform::windows::IconExtWindows;
        Icon::from_resource(1, None).ok()
    };
    #[cfg(not(windows))]
    let icon: Option<Icon> = None;
    let geometry_path = dir.join("window.json");
    let saved = load_geometry(&geometry_path);
    let builder = WindowBuilder::new().with_title(TITLE).with_min_inner_size(LogicalSize::new(960.0, 620.0)).with_window_icon(icon);
    let window = place_window(builder, &event_loop, saved).build(&event_loop).map_err(|e| e.to_string())?;
    let mut normal = normal_geometry(&window).or(saved.filter(|g| g.width >= 640 && g.height >= 420));
    let mut maximized = window.is_maximized();
    let window = Rc::new(window);
    let origin = format!("http://127.0.0.1:{port}");
    let mut context = WebContext::new(Some(dir.join("webview")));
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(format!("{origin}/"))
        .with_background_color((244, 241, 235, 255))
        .with_devtools(cfg!(debug_assertions))
        .with_navigation_handler({
            let origin = origin.clone();
            move |url| {
                let inside = url.starts_with(&origin) || url.starts_with("about:") || url.starts_with("blob:") || url.starts_with("data:");
                if !inside {
                    open_external(&url);
                }
                inside
            }
        })
        .with_new_window_req_handler(|url, _| {
            open_external(&url);
            NewWindowResponse::Deny
        })
        .with_document_title_changed_handler({
            let window = window.clone();
            move |title| window.set_title(&title)
        })
        .with_ipc_handler({
            let proxy = proxy.clone();
            move |req| {
                if req.body() == "closed" {
                    let _ = proxy.send_event(UserEvent::Close);
                }
            }
        })
        .with_download_completed_handler({
            let proxy = proxy.clone();
            move |_, path, ok| {
                if let (true, Some(path)) = (ok, path) {
                    let _ = proxy.send_event(UserEvent::Downloaded(path));
                }
            }
        });
    // 排查问题用：设置 AI_NOVEL_DEVTOOLS_PORT 后可以用 Edge/Chrome 的 DevTools 连到这个端口
    #[cfg(windows)]
    let builder = match std::env::var("AI_NOVEL_DEVTOOLS_PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
        Some(p) => {
            use wry::WebViewBuilderExtWindows;
            builder.with_devtools(true).with_additional_browser_args(format!("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={p}"))
        }
        None => builder,
    };
    let webview = builder.build(&*window).map_err(|e| e.to_string())?;
    let mut closing = false;
    let mut geometry_changed = false;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &context;
        match event {
            Event::WindowEvent { event: WindowEvent::Moved(_) | WindowEvent::Resized(_), .. } => geometry_changed = true,
            // Moved 比 tao 更新最大化标记来得早，等这一批消息处理完再读，不然会把最大化后的尺寸当成普通尺寸
            Event::MainEventsCleared if geometry_changed => {
                geometry_changed = false;
                if !window.is_minimized() {
                    maximized = window.is_maximized();
                    normal = normal_geometry(&window).or(normal);
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if closing {
                    *flow = ControlFlow::Exit;
                    return;
                }
                closing = true;
                save_geometry(&geometry_path, &window, normal, maximized);
                // 先让页面保存正在编辑的内容，保存完页面会发来 closed；最多等 3 秒
                let _ = webview.evaluate_script(
                    "Promise.resolve(window.__aiNovelFlush && window.__aiNovelFlush()).catch(() => {}).finally(() => window.ipc.postMessage('closed'))",
                );
                let proxy = proxy.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(3));
                    let _ = proxy.send_event(UserEvent::Close);
                });
            }
            Event::UserEvent(UserEvent::Close) => *flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Downloaded(path)) => {
                tracing::info!("已下载：{}", path.display());
                let text = serde_json::to_string(&format!("已保存到：{}", path.display())).unwrap_or_default();
                let _ = webview.evaluate_script(&format!("window.__aiNovelToast && window.__aiNovelToast({text}, 'info', 8000)"));
            }
            _ => {}
        }
    })
}

/// `--serve`：只开本地服务、不开窗口，开机自启后给手机用。可加 `--port 端口`、`--password 密码`
struct ServeArgs {
    port: u16,
    password: Option<String>,
}

fn serve_args() -> Option<ServeArgs> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--serve") {
        return None;
    }
    let value = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    Some(ServeArgs {
        port: value("--port").and_then(|p| p.parse().ok()).unwrap_or(SERVE_PORT),
        password: value("--password").or_else(|| std::env::var("AI_NOVEL_PASSWORD").ok()).filter(|p| !p.trim().is_empty()),
    })
}

fn serve_only(db: Db, args: ServeArgs) {
    let listener = match TcpListener::bind(("127.0.0.1", args.port)) {
        Ok(l) => l,
        Err(_) if already_running(args.port) => {
            tracing::info!("后台服务已经在运行（端口 {}）", args.port);
            return;
        }
        Err(e) => {
            tracing::error!("后台服务无法监听端口 {}：{e}", args.port);
            return;
        }
    };
    tracing::info!("后台服务已启动：http://127.0.0.1:{}{}", args.port, if args.password.is_some() { "（已设访问密码）" } else { "" });
    let state = AppState::new(db, args.password);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("启动异步运行时");
    rt.block_on(async move {
        listener.set_nonblocking(true).expect("设置非阻塞");
        let listener = tokio::net::TcpListener::from_std(listener).expect("监听端口");
        if let Err(e) = axum::serve(listener, build_router(state)).await {
            tracing::error!("后台服务退出：{e}");
        }
    });
}

fn main() {
    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        message(&format!("无法创建数据目录 {}：{e}", dir.display()));
        return;
    }
    init_log(&dir);
    let db = match Db::open(&dir.join("ai-novel.db")) {
        Ok(db) => db,
        Err(e) => {
            message(&format!("打开数据库失败：{e:#}"));
            return;
        }
    };
    if let Some(args) = serve_args() {
        serve_only(db, args);
        return;
    }
    let port = match start_server(db) {
        Ok(p) => p,
        Err(e) => {
            message(&e);
            return;
        }
    };
    tracing::info!("桌面版已启动：http://127.0.0.1:{port}（数据目录：{}）", dir.display());
    if let Err(e) = run_window(port, &dir) {
        tracing::error!("打开窗口失败：{e}");
        let url = format!("http://127.0.0.1:{port}/");
        open_external(&url);
        message(&format!(
            "没能打开程序窗口（{e}），已经改用浏览器打开 {url}\n\n可能是缺少 WebView2 运行时，可以从 https://go.microsoft.com/fwlink/p/?LinkId=2124703 下载安装。\n\n关闭这个提示框，程序就会退出。"
        ));
    }
}
