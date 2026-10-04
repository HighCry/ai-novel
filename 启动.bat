@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist "target\release\ai-novel.exe" (
  echo 第一次运行，正在编译，需要几分钟……
  cargo build --release --bin ai-novel
  if errorlevel 1 (
    echo 编译失败，请确认已安装 Rust：https://rustup.rs
    pause
    exit /b 1
  )
)
echo 启动 AI 小说工坊，浏览器会自动打开 http://127.0.0.1:8686 ，关闭这个窗口即退出。
"target\release\ai-novel.exe" %*
pause
