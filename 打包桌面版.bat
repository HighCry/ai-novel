@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo 正在编译桌面版，第一次需要几分钟……
cargo build --release --features desktop --bin ai-novel-desktop
if errorlevel 1 (
  echo 编译失败，请确认已安装 Rust：https://rustup.rs
  pause
  exit /b 1
)
rem 手机访问的后台服务（--serve）也占着这个 exe，复制前先停掉，复制完再开
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='AI小说工坊.exe'\" | Where-Object CommandLine -like '*--serve*' | ForEach-Object { Stop-Process -Id $_.ProcessId -Force; Wait-Process -Id $_.ProcessId -Timeout 5 -ErrorAction SilentlyContinue }"
copy /y "target\release\ai-novel-desktop.exe" "AI小说工坊.exe" >nul
if errorlevel 1 (
  echo 复制失败：AI小说工坊.exe 可能正开着，关掉它再运行一次。
  pause
  exit /b 1
)
if exist "%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\AI小说工坊（手机访问）.lnk" start "" "AI小说工坊.exe" --serve
echo 完成。双击 AI小说工坊.exe 打开，也可以把它拷到任何地方单独使用。
pause
