//! Android JNI 入口：在 APP 进程内启动 ai-novel 的 axum 服务，监听 127.0.0.1。
//! 前端 WebView 直接访问 http://127.0.0.1:<port>，数据存在 APP 私有目录。

use jni::objects::{JClass, JString};
use jni::sys::jint;
use jni::JNIEnv;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

/// 对应 Java: com.ainovel.app.NativeServer.start(String dataDir, int port)
/// 返回 0 表示已启动（或此前已启动），-1 表示参数错误。
#[no_mangle]
pub extern "system" fn Java_com_ainovel_app_NativeServer_start(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    port: jint,
) -> jint {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("ainovel"),
    );

    // 同一进程只启动一次；重复调用直接返回。
    if STARTED.swap(true, Ordering::SeqCst) {
        return 0;
    }

    let dir: String = match env.get_string(&data_dir) {
        Ok(s) => s.into(),
        Err(_) => {
            STARTED.store(false, Ordering::SeqCst);
            return -1;
        }
    };
    let port = port as u16;

    std::thread::Builder::new()
        .name("ainovel-server".into())
        .spawn(move || {
            if let Err(e) = run(&dir, port) {
                log::error!("ai-novel 服务退出：{e:#}");
            }
        })
        .ok();

    0
}

fn run(data_dir: &str, port: u16) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        std::fs::create_dir_all(data_dir)?;
        let db_path = Path::new(data_dir).join("ai-novel.db");
        let db = ai_novel::db::Db::open(&db_path)?;
        let state = ai_novel::state::AppState::new(db, None);
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        log::info!("ai-novel 已在 http://127.0.0.1:{port} 启动");
        axum::serve(listener, ai_novel::build_router(state)).await?;
        Ok::<(), anyhow::Error>(())
    })
}
