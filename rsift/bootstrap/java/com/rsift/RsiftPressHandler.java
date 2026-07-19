package com.rsift;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;

/** JDK-only click bridge — no Minecraft compile-time dependency. */
public final class RsiftPressHandler implements InvocationHandler {
    private final int buttonId;

    public RsiftPressHandler(int buttonId) {
        this.buttonId = buttonId;
    }

    @Override
    public Object invoke(Object proxy, Method method, Object[] args) {
        if (method != null && "onPress".equals(method.getName())) {
            nativeOnButton(buttonId);
        }
        return null;
    }

    private static native void nativeOnButton(int id);
}
