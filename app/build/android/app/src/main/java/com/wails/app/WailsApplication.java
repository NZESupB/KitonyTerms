package com.wails.app;

import android.app.Application;

/** 进程级Go桥接的所有者，不把Activity重建视为应用退出。 */
public class WailsApplication extends Application {
    private WailsBridge bridge;

    WailsBridge bridge() {
        if (bridge == null) bridge = new WailsBridge(this);
        return bridge;
    }
}
