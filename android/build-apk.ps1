# 用 Android SDK build-tools 手工打包 APK（不依赖 Gradle / 不联网）。
# 前置：已用 cargo ndk 编出 jni\out\arm64-v8a\libainovel.so（x86_64 可选，模拟器用）。
# 版本：versionName 取自上级 Cargo.toml 的 [package]；versionCode 默认按 主*10000+次*100+修订 推算（0.5.0 → 500），
#       同一 versionName 要重发时用 -VersionCode 指定更大的值——安卓不允许用更小的 versionCode 覆盖安装。
# 产物：android\AI小说工坊.apk（最新一版），以及 android\dist\v<版本>-<versionCode>\ 下的归档 APK、SHA256SUMS.txt、build-info.txt。
param([int]$VersionCode = 0)

# 用 Continue：Android 工具会往 stderr 打印提示/进度，不能当成终止错误；
# 真正的失败靠每步后的 $LASTEXITCODE 检查 + 显式 throw 捕获。
$ErrorActionPreference = 'Continue'

$SDK    = 'E:\Android\Sdk'
$BT     = Join-Path $SDK 'build-tools\35.0.0'
$ANDJAR = Join-Path $SDK 'platforms\android-35\android.jar'
$JAVA   = $env:JAVA_HOME
if (-not $JAVA) { $JAVA = 'C:\Program Files\Microsoft\jdk-21.0.11.10-hotspot\' }

$ROOT = Split-Path -Parent $MyInvocation.MyCommand.Path   # ...\ai-novel\android
$REPO = Split-Path -Parent $ROOT
$APP  = Join-Path $ROOT 'app'
$MAIN = Join-Path $APP 'src\main'
$OUT  = Join-Path $ROOT 'jni\out'
$B    = Join-Path $APP 'build'
$NAME = 'AI小说工坊'

$cargo = Get-Content -Raw -Encoding UTF8 (Join-Path $REPO 'Cargo.toml')
$m = [regex]::Match($cargo, '(?ms)^\[package\][^\[]*?^version\s*=\s*"((\d+)\.(\d+)\.(\d+)[^"]*)"')
if (-not $m.Success) { throw '无法从 Cargo.toml 的 [package] 读取 version' }
$VersionName = $m.Groups[1].Value
if ($VersionCode -le 0) { $VersionCode = [int]$m.Groups[2].Value * 10000 + [int]$m.Groups[3].Value * 100 + [int]$m.Groups[4].Value }

# 收集已交叉编译出的 ABI：arm64-v8a 必需（真机），x86_64 可选（模拟器）。
$ABIS = @()
foreach ($abi in 'arm64-v8a','x86_64') {
    if (Test-Path (Join-Path $OUT "$abi\libainovel.so")) { $ABIS += $abi }
}
if ($ABIS -notcontains 'arm64-v8a') { throw "缺少原生库：$OUT\arm64-v8a\libainovel.so（先运行 cargo ndk 交叉编译）" }
if (!(Test-Path $ANDJAR)) { throw "缺少 android.jar：$ANDJAR" }
Write-Host "版本 $VersionName（versionCode $VersionCode），打包 ABI: $($ABIS -join ', ')"

# 原生库比 Rust 源码旧时只警告不中断：git checkout 等操作也会刷新 mtime，可能误报。
$srcNewest = @(Get-ChildItem -Recurse -File (Join-Path $REPO 'src'), (Join-Path $ROOT 'jni\src') -Filter *.rs -ErrorAction SilentlyContinue) +
             @(Get-Item (Join-Path $REPO 'Cargo.lock'), (Join-Path $ROOT 'jni\Cargo.lock') -ErrorAction SilentlyContinue) |
             Sort-Object LastWriteTime -Descending | Select-Object -First 1
foreach ($abi in $ABIS) {
    $so = Get-Item (Join-Path $OUT "$abi\libainovel.so")
    if ($srcNewest -and $so.LastWriteTime -lt $srcNewest.LastWriteTime) {
        Write-Warning "$abi 的 libainovel.so（$($so.LastWriteTime)）比 $($srcNewest.Name)（$($srcNewest.LastWriteTime)）旧，APK 可能不含最新改动；先在 android\jni 下运行 cargo ndk -o out -t $abi -P 24 build --release"
    }
}

Remove-Item -Recurse -Force $B -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$B\classes","$B\dex","$B\staging" | Out-Null
foreach ($abi in $ABIS) { New-Item -ItemType Directory -Force -Path "$B\staging\lib\$abi" | Out-Null }

Write-Host '[1/8] javac 编译 Java'
$javac = Join-Path $JAVA 'bin\javac.exe'
$javaFiles = Get-ChildItem -Recurse "$MAIN\java" -Filter *.java | ForEach-Object { $_.FullName }
& $javac -source 8 -target 8 -encoding UTF-8 -Xlint:-options -classpath $ANDJAR -d "$B\classes" $javaFiles
if ($LASTEXITCODE) { throw 'javac 失败' }

Write-Host '[2/8] d8 生成 dex'
$classFiles = Get-ChildItem -Recurse "$B\classes" -Filter *.class | ForEach-Object { $_.FullName }
& (Join-Path $BT 'd8.bat') --release --min-api 24 --lib $ANDJAR --output "$B\dex" $classFiles
if ($LASTEXITCODE) { throw 'd8 失败' }

Write-Host '[3/8] aapt2 compile 编译资源（启动图标）'
& (Join-Path $BT 'aapt2.exe') compile --dir "$MAIN\res" -o "$B\res.zip"
if ($LASTEXITCODE) { throw 'aapt2 compile 失败' }

Write-Host '[4/8] aapt2 link 打包清单与资源'
& (Join-Path $BT 'aapt2.exe') link -o "$B\base.apk" -I $ANDJAR --manifest "$MAIN\AndroidManifest.xml" `
    --min-sdk-version 24 --target-sdk-version 35 --version-code $VersionCode --version-name $VersionName "$B\res.zip"
if ($LASTEXITCODE) { throw 'aapt2 link 失败' }

Write-Host '[5/8] 塞入 classes.dex 和各 ABI 的 libainovel.so'
Copy-Item "$B\dex\classes.dex" "$B\staging\classes.dex" -Force
foreach ($abi in $ABIS) { Copy-Item (Join-Path $OUT "$abi\libainovel.so") "$B\staging\lib\$abi\libainovel.so" -Force }
Copy-Item "$B\base.apk" "$B\unsigned.apk" -Force
Push-Location "$B\staging"
& (Join-Path $JAVA 'bin\jar.exe') uf "$B\unsigned.apk" classes.dex lib
$jarCode = $LASTEXITCODE
Pop-Location
if ($jarCode) { throw 'jar 追加失败' }

Write-Host '[6/8] zipalign 对齐'
& (Join-Path $BT 'zipalign.exe') -f -p 4 "$B\unsigned.apk" "$B\aligned.apk"
if ($LASTEXITCODE) { throw 'zipalign 失败' }

Write-Host '[7/8] 调试签名'
$ks = Join-Path $ROOT 'debug.keystore'
if (!(Test-Path $ks)) {
    & (Join-Path $JAVA 'bin\keytool.exe') -genkeypair -keystore $ks -storepass android -keypass android `
        -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 10000 -dname 'CN=AI Novel Debug, O=ai-novel, C=CN'
    if ($LASTEXITCODE) { throw 'keytool 生成调试证书失败' }
}
$out = Join-Path $B "$NAME.apk"
& (Join-Path $BT 'apksigner.bat') sign --ks $ks --ks-pass pass:android --key-pass pass:android --min-sdk-version 24 --out $out "$B\aligned.apk"
if ($LASTEXITCODE) { throw 'apksigner 签名失败' }

Write-Host '[8/8] 校验、复制并归档'
$certs = & (Join-Path $BT 'apksigner.bat') verify --print-certs $out
if ($LASTEXITCODE) { throw 'apksigner 校验失败' }
$certs | Write-Host
Copy-Item $out (Join-Path $ROOT "$NAME.apk") -Force

$rel     = Join-Path $ROOT "dist\v$VersionName-$VersionCode"
$apkName = "$NAME-v$VersionName-$VersionCode.apk"
New-Item -ItemType Directory -Force -Path $rel | Out-Null
Copy-Item $out (Join-Path $rel $apkName) -Force
$sha = (Get-FileHash -Algorithm SHA256 (Join-Path $rel $apkName)).Hash.ToLower()

$commit = (git -C $REPO rev-parse --short HEAD 2>$null)
if (-not $commit) { $commit = 'unknown' } elseif (git -C $REPO status --porcelain 2>$null) { $commit += '（工作区有未提交改动）' }
$dn     = ($certs | Select-String 'certificate DN: (.+)$' | Select-Object -First 1).Matches.Groups[1].Value
$certSha = ($certs | Select-String 'certificate SHA-256 digest: ([0-9a-f]+)' | Select-Object -First 1).Matches.Groups[1].Value

# sha256sum -c 要求无 BOM、LF 换行，所以不用 Set-Content（PS 5.1 的 UTF8 会带 BOM）。
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText((Join-Path $rel 'SHA256SUMS.txt'), "$sha  $apkName`n", $utf8)
$info = @(
    "应用        $NAME（com.ainovel.app）",
    "versionName $VersionName",
    "versionCode $VersionCode",
    "ABI         $($ABIS -join ', ')",
    "SDK         min 24 / target 35",
    "源码提交    $commit",
    "构建时间    $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')",
    "APK SHA256  $sha",
    "签名证书    $dn（调试证书，仅供自用侧载，不能上架应用商店）",
    "证书SHA256  $certSha"
)
[System.IO.File]::WriteAllText((Join-Path $rel 'build-info.txt'), (($info -join "`n") + "`n"), $utf8)

Write-Host "APK 完成：$(Join-Path $ROOT "$NAME.apk")"
Write-Host "归档：$rel"
Write-Host "SHA256：$sha"
