package com.ainovel.app;

/** 加载 Rust 原生库，并在 APP 进程内启动 ai-novel 的 axum 服务。 */
public final class NativeServer {
    static {
        System.loadLibrary("ainovel");
    }

    private NativeServer() {}

    /**
     * 启动内置服务，监听 127.0.0.1:port。重复调用安全（原生侧只会启动一次）。
     *
     * @param dataDir 数据目录（SQLite 等），应为 APP 私有可写目录
     * @param port    监听端口
     * @return 0 表示已启动或此前已启动，-1 表示参数错误
     */
    public static native int start(String dataDir, int port);
}
