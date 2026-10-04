package com.ainovel.app;

import android.app.Activity;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.TypedArray;
import android.graphics.Color;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.webkit.JavascriptInterface;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import android.widget.Toast;

import java.io.File;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.util.Locale;

/**
 * 单 Activity 的 WebView 壳：先在进程内拉起内置服务，再用 WebView 打开
 * http://127.0.0.1:PORT 。整套应用（前端 + 数据库）都在本机，离线可用。
 *
 * 网页铺满全屏、画到状态栏和导航栏下面，两条栏的高度通过 CSS 变量 --safe-top / --safe-bottom 交给网页自己让位；
 * 键盘弹出时由这里把 WebView 底边抬到键盘上方。
 */
public class MainActivity extends Activity {

    private static final int PORT = 18688;
    private static final int FILE_CHOOSER_REQUEST = 1;

    private FrameLayout root;
    private WebView web;
    private ValueCallback<Uri[]> filePathCallback;
    private SharedPreferences prefs;
    // JS 桥在 WebView 自己的线程里读这两个值
    private volatile float safeTop;
    private volatile float safeBottom;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        File dataDir = new File(getFilesDir(), "ai-novel");
        if (!dataDir.exists()) {
            dataDir.mkdirs();
        }
        NativeServer.start(dataDir.getAbsolutePath(), PORT);

        // 记住上次网页的主题色，深色主题启动时不先闪一下浅色。打包脚本不生成 R 类，默认色从主题的 windowBackground 读
        TypedArray theme = getTheme().obtainStyledAttributes(new int[]{android.R.attr.windowBackground});
        int paper = theme.getColor(0, Color.WHITE);
        theme.recycle();
        prefs = getSharedPreferences("shell", MODE_PRIVATE);
        int bg = prefs.getInt("bg", paper);
        final boolean dark = prefs.getBoolean("dark", false);

        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
        }
        root = new FrameLayout(this);
        root.setBackgroundColor(bg);
        web = new WebView(this);
        web.setBackgroundColor(bg);
        root.addView(web, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            applyInsets(insets);
            return Build.VERSION.SDK_INT >= 30 ? WindowInsets.CONSUMED : insets.consumeSystemWindowInsets();
        });

        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setDatabaseEnabled(true);
        s.setAllowFileAccess(true);
        s.setMediaPlaybackRequiresUserGesture(false);
        web.addJavascriptInterface(new Bridge(), "AiNovelAndroid");

        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                // 只在内置服务里导航，外链交给系统浏览器
                String url = request.getUrl().toString();
                if (url.startsWith("http://127.0.0.1:" + PORT)) {
                    return false;
                }
                try {
                    startActivity(new Intent(Intent.ACTION_VIEW, request.getUrl()));
                } catch (Exception ignored) {
                }
                return true;
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                // 服务还没起好时会连接失败，隔一会儿重试，直到拉起成功
                if (request.isForMainFrame()) {
                    view.postDelayed(new Runnable() {
                        @Override
                        public void run() {
                            view.loadUrl(home());
                        }
                    }, 500);
                }
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                pushSafeArea();
            }
        });

        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onShowFileChooser(WebView view, ValueCallback<Uri[]> callback,
                                             FileChooserParams params) {
                filePathCallback = callback;
                try {
                    startActivityForResult(params.createIntent(), FILE_CHOOSER_REQUEST);
                } catch (Exception e) {
                    filePathCallback = null;
                    return false;
                }
                return true;
            }
        });

        web.setDownloadListener((url, userAgent, contentDisposition, mimetype, contentLength) -> {
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
            } catch (Exception e) {
                Toast.makeText(MainActivity.this, "无法打开下载：" + url, Toast.LENGTH_SHORT).show();
            }
        });

        setContentView(root);
        setSystemBars(dark);
        // 窗口挂上之后主题里的 windowLightStatusBar 会再生效一次，这里再按记住的主题改回来
        root.post(() -> setSystemBars(dark));
        loadWhenReady();
    }

    private String home() {
        return "http://127.0.0.1:" + PORT + "/";
    }

    /** 等内置服务开始监听再加载，免得先闪出 WebView 的「无法连接」错误页 */
    private void loadWhenReady() {
        new Thread(() -> {
            for (int i = 0; i < 100; i++) {
                try (Socket socket = new Socket()) {
                    socket.connect(new InetSocketAddress("127.0.0.1", PORT), 200);
                    break;
                } catch (IOException e) {
                    try {
                        Thread.sleep(100);
                    } catch (InterruptedException ie) {
                        return;
                    }
                }
            }
            runOnUiThread(() -> {
                if (!isDestroyed()) {
                    web.loadUrl(home());
                }
            });
        }, "wait-server").start();
    }

    private void applyInsets(WindowInsets insets) {
        int top;
        int bottom;
        int left;
        int right;
        int ime;
        if (Build.VERSION.SDK_INT >= 30) {
            Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            top = bars.top;
            bottom = bars.bottom;
            left = bars.left;
            right = bars.right;
            ime = insets.getInsets(WindowInsets.Type.ime()).bottom;
        } else {
            top = insets.getSystemWindowInsetTop();
            left = insets.getSystemWindowInsetLeft();
            right = insets.getSystemWindowInsetRight();
            bottom = insets.getStableInsetBottom();
            int all = insets.getSystemWindowInsetBottom();
            ime = all > bottom ? all : 0;
        }
        boolean keyboard = ime > bottom;
        // 左右（横屏时的刘海、侧边导航条）和键盘由原生让开；上下两条栏交给网页
        root.setPadding(left, 0, right, keyboard ? ime : 0);
        float density = getResources().getDisplayMetrics().density;
        safeTop = top / density;
        safeBottom = keyboard ? 0 : bottom / density;
        pushSafeArea();
    }

    private void pushSafeArea() {
        web.evaluateJavascript(String.format(Locale.US,
                "(function(s){s.setProperty('--safe-top','%.1fpx');s.setProperty('--safe-bottom','%.1fpx')})"
                        + "(document.documentElement.style)", safeTop, safeBottom), null);
    }

    /** 浅色主题配深色状态栏图标，深色主题反过来 */
    private void setSystemBars(boolean dark) {
        Window window = getWindow();
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController c = window.getInsetsController();
            if (c != null) {
                int light = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                        | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                c.setSystemBarsAppearance(dark ? 0 : light, light);
            }
        } else {
            int flags = View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION;
            if (!dark) {
                flags |= View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR;
                if (Build.VERSION.SDK_INT >= 26) {
                    flags |= View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
                }
            }
            window.getDecorView().setSystemUiVisibility(flags);
        }
    }

    /** 给网页调用的接口（window.AiNovelAndroid） */
    private final class Bridge {
        @JavascriptInterface
        public String insets() {
            return String.format(Locale.US, "{\"top\":%.1f,\"bottom\":%.1f}", safeTop, safeBottom);
        }

        @JavascriptInterface
        public void setTheme(boolean dark, String bg) {
            final int color;
            try {
                color = Color.parseColor(bg);
            } catch (RuntimeException e) {
                return;
            }
            prefs.edit().putInt("bg", color).putBoolean("dark", dark).apply();
            runOnUiThread(() -> {
                root.setBackgroundColor(color);
                web.setBackgroundColor(color);
                setSystemBars(dark);
            });
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == FILE_CHOOSER_REQUEST) {
            if (filePathCallback != null) {
                Uri[] results = null;
                if (resultCode == RESULT_OK && data != null) {
                    if (data.getDataString() != null) {
                        results = new Uri[]{Uri.parse(data.getDataString())};
                    } else if (data.getClipData() != null) {
                        int n = data.getClipData().getItemCount();
                        results = new Uri[n];
                        for (int i = 0; i < n; i++) {
                            results[i] = data.getClipData().getItemAt(i).getUri();
                        }
                    }
                }
                filePathCallback.onReceiveValue(results);
                filePathCallback = null;
            }
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }

    @Override
    public void onBackPressed() {
        // 先交给网页：收菜单、关最上层的浮层、从书里回书架。网页没处理（已在书架）时退到后台而不是销毁，
        // 进程里的服务和页面都留着，回来不用重新加载
        web.evaluateJavascript("window.__aiNovelBack ? window.__aiNovelBack() : false", handled -> {
            if (!"true".equals(handled)) {
                moveTaskToBack(true);
            }
        });
    }
}
