# 用 Android SDK build-tools 手工打包 APK（不依赖 Gradle / 不联网）。
# 前置：已用 cargo ndk 编出 jni\out\arm64-v8a\libainovel.so
# 用 Continue：Android 工具会往 stderr 打印提示/进度，不能当成终止错误；
# 真正的失败靠每步后的 $LASTEXITCODE 检查 + 显式 throw 捕获。
$ErrorActionPreference = 'Continue'

$SDK    = 'E:\Android\Sdk'
$BT     = Join-Path $SDK 'build-tools\35.0.0'
$ANDJAR = Join-Path $SDK 'platforms\android-35\android.jar'
$JAVA   = $env:JAVA_HOME
if (-not $JAVA) { $JAVA = 'C:\Program Files\Microsoft\jdk-21.0.11.10-hotspot\' }

$ROOT = Split-Path -Parent $MyInvocation.MyCommand.Path   # ...\ai-novel\android
$APP  = Join-Path $ROOT 'app'
$MAIN = Join-Path $APP 'src\main'
$SO   = Join-Path $ROOT 'jni\out\arm64-v8a\libainovel.so'
$B    = Join-Path $APP 'build'

if (!(Test-Path $SO))     { throw "缺少原生库：$SO（先运行 cargo ndk 交叉编译）" }
if (!(Test-Path $ANDJAR)) { throw "缺少 android.jar：$ANDJAR" }

Remove-Item -Recurse -Force $B -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$B\classes","$B\dex","$B\staging\lib\arm64-v8a" | Out-Null

Write-Host '[1/7] javac 编译 Java'
$javac = Join-Path $JAVA 'bin\javac.exe'
$javaFiles = Get-ChildItem -Recurse "$MAIN\java" -Filter *.java | ForEach-Object { $_.FullName }
& $javac -source 8 -target 8 -encoding UTF-8 -Xlint:-options -classpath $ANDJAR -d "$B\classes" $javaFiles
if ($LASTEXITCODE) { throw 'javac 失败' }

Write-Host '[2/7] d8 生成 dex'
$classFiles = Get-ChildItem -Recurse "$B\classes" -Filter *.class | ForEach-Object { $_.FullName }
& (Join-Path $BT 'd8.bat') --release --min-api 24 --lib $ANDJAR --output "$B\dex" $classFiles
if ($LASTEXITCODE) { throw 'd8 失败' }

Write-Host '[3/7] aapt2 link 打包清单与资源'
& (Join-Path $BT 'aapt2.exe') link -o "$B\base.apk" -I $ANDJAR --manifest "$MAIN\AndroidManifest.xml" --min-sdk-version 24 --target-sdk-version 35
if ($LASTEXITCODE) { throw 'aapt2 link 失败' }

Write-Host '[4/7] 塞入 classes.dex 和 libainovel.so'
Copy-Item "$B\dex\classes.dex" "$B\staging\classes.dex" -Force
Copy-Item $SO "$B\staging\lib\arm64-v8a\libainovel.so" -Force
Copy-Item "$B\base.apk" "$B\unsigned.apk" -Force
Push-Location "$B\staging"
& (Join-Path $JAVA 'bin\jar.exe') uf "$B\unsigned.apk" classes.dex lib
$jarCode = $LASTEXITCODE
Pop-Location
if ($jarCode) { throw 'jar 追加失败' }

Write-Host '[5/7] zipalign 对齐'
& (Join-Path $BT 'zipalign.exe') -f -p 4 "$B\unsigned.apk" "$B\aligned.apk"
if ($LASTEXITCODE) { throw 'zipalign 失败' }

Write-Host '[6/7] 调试签名'
$ks = Join-Path $ROOT 'debug.keystore'
if (!(Test-Path $ks)) {
    & (Join-Path $JAVA 'bin\keytool.exe') -genkeypair -keystore $ks -storepass android -keypass android `
        -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 10000 -dname 'CN=AI Novel Debug, O=ai-novel, C=CN'
    if ($LASTEXITCODE) { throw 'keytool 生成调试证书失败' }
}
$out = Join-Path $B 'AI小说工坊.apk'
& (Join-Path $BT 'apksigner.bat') sign --ks $ks --ks-pass pass:android --key-pass pass:android --min-sdk-version 24 --out $out "$B\aligned.apk"
if ($LASTEXITCODE) { throw 'apksigner 签名失败' }

Write-Host '[7/7] 校验并复制到 android 根目录'
& (Join-Path $BT 'apksigner.bat') verify --print-certs $out
Copy-Item $out (Join-Path $ROOT 'AI小说工坊.apk') -Force
Write-Host "APK 完成：$(Join-Path $ROOT 'AI小说工坊.apk')"
