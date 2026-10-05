use ai_novel::{build_router, db::Db, state::AppState};
use anyhow::Context;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

const HELP: &str = "AI 小说工坊

用法：ai-novel [选项]

选项：
  --host <地址>       监听地址，默认 127.0.0.1（局域网访问用 0.0.0.0，务必同时设置密码）
  --port <端口>       端口，默认 8686
  --data-dir <目录>   数据目录，默认 ./data
  --password <密码>   访问密码（浏览器会弹出登录框，用户名随意）
  --no-browser        启动后不自动打开浏览器
  -h, --help          显示帮助

也可以用环境变量：AI_NOVEL_HOST、AI_NOVEL_PORT、AI_NOVEL_DATA、AI_NOVEL_PASSWORD、AI_NOVEL_NO_BROWSER";

struct Args {
    host: String,
    port: u16,
    data_dir: PathBuf,
    password: Option<String>,
    no_browser: bool,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn parse_args() -> Args {
    let mut a = Args {
        host: env("AI_NOVEL_HOST").unwrap_or_else(|| "127.0.0.1".into()),
        port: env("AI_NOVEL_PORT").and_then(|p| p.parse().ok()).unwrap_or(8686),
        data_dir: PathBuf::from(env("AI_NOVEL_DATA").unwrap_or_else(|| "data".into())),
        password: env("AI_NOVEL_PASSWORD"),
        no_browser: env("AI_NOVEL_NO_BROWSER").is_some(),
    };
    let mut it = std::env::args().skip(1);
    let value = |it: &mut std::iter::Skip<std::env::Args>, name: &str| {
        it.next().unwrap_or_else(|| {
            eprintln!("{name} 需要一个值\n\n{HELP}");
            std::process::exit(2)
        })
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--host" => a.host = value(&mut it, "--host"),
            "--port" => {
                a.port = value(&mut it, "--port").parse().unwrap_or_else(|_| {
                    eprintln!("--port 需要一个数字");
                    std::process::exit(2)
                })
            }
            "--data-dir" => a.data_dir = PathBuf::from(value(&mut it, "--data-dir")),
            "--password" => a.password = Some(value(&mut it, "--password")),
            "--no-browser" => a.no_browser = true,
            "-h" | "--help" => {
                println!("{HELP}");
                std::process::exit(0)
            }
            other => {
                eprintln!("未知参数：{other}\n\n{HELP}");
                std::process::exit(2)
            }
        }
    }
    a
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        tracing::warn!("没能自动打开浏览器：{e}，请手动访问 {url}");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let args = parse_args();
    std::fs::create_dir_all(&args.data_dir).with_context(|| format!("创建数据目录 {} 失败", args.data_dir.display()))?;
    let db = Db::open(&args.data_dir.join("ai-novel.db")).context("打开数据库失败")?;
    let local_only = matches!(args.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if !local_only && args.password.is_none() {
        tracing::warn!("正在监听 {}，但没有设置密码：局域网内任何人都能访问你的作品和接口密钥。建议加上 --password", args.host);
    }
    let state = AppState::new(db, args.password.clone());
    tokio::spawn(ai_novel::backup::auto_loop(state.db.clone()));
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port))
        .await
        .with_context(|| format!("无法监听 {}:{}，端口可能被占用，可以用 --port 换一个", args.host, args.port))?;
    let shown_host = if args.host == "0.0.0.0" { "127.0.0.1" } else { args.host.as_str() };
    let url = format!("http://{shown_host}:{}", args.port);
    tracing::info!("AI 小说工坊已启动：{url}（数据目录：{}）", args.data_dir.display());
    if !args.no_browser {
        open_browser(&url);
    }
    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
