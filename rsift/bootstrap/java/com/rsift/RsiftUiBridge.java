package com.rsift;

import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;

/**
 * Runtime UI injection using mapped Minecraft class names (official 1.21.11).
 * Compiled without the obfuscated client jar.
 */
public final class RsiftUiBridge {
    private RsiftUiBridge() {}

    public static void injectButtons(Object screen, String screenClass, ClassLoader gameLoader) throws ReflectiveOperationException {
        nativePrepareScreen(screenClass);
        int count = nativeButtonCount(screenClass);
        if (count <= 0) {
            nativeLog("[RsiftUiBridge] no buttons registered for " + screenClass);
            return;
        }

        Class<?> screenCls = screen.getClass();
        Class<?> buttonCls = Class.forName("net.minecraft.client.gui.components.Button", true, gameLoader);
        Class<?> componentCls = Class.forName("net.minecraft.network.chat.Component", true, gameLoader);
        Class<?> onPressCls = Class.forName("net.minecraft.client.gui.components.Button$OnPress", true, gameLoader);
        Class<?> widgetCls = Class.forName("net.minecraft.client.gui.components.AbstractWidget", true, gameLoader);

        Method literal = componentCls.getMethod("literal", String.class);
        Method builder = buttonCls.getMethod("builder", componentCls, onPressCls);
        Method addWidget = screenCls.getMethod("addRenderableWidget", widgetCls);
        Method widthM = screenCls.getMethod("width");
        Method heightM = screenCls.getMethod("height");
        int sw = ((Number) widthM.invoke(screen)).intValue();
        int sh = ((Number) heightM.invoke(screen)).intValue();

        Method bounds = null;
        Method build = null;
        int added = 0;

        for (int i = 0; i < count; i++) {
            int id = nativeButtonId(screenClass, i);
            String label = nativeButtonLabel(screenClass, i);
            int x = nativeButtonX(screenClass, i);
            int y = nativeButtonY(screenClass, i);
            int w = nativeButtonW(screenClass, i);
            int h = nativeButtonH(screenClass, i);

            if (screenClass.contains("TitleScreen")) {
                x = sw / 2 + 10;
                y = sh / 4 + 72 + i * 24;
            }

            Object text = literal.invoke(null, label);
            InvocationHandler handler = new RsiftPressHandler(id);
            Object onPress = Proxy.newProxyInstance(gameLoader, new Class<?>[] { onPressCls }, handler);
            Object builderObj = builder.invoke(null, text, onPress);

            if (bounds == null) {
                bounds = builderObj.getClass().getMethod("bounds", int.class, int.class, int.class, int.class);
                build = builderObj.getClass().getMethod("build");
            }
            Object bounded = bounds.invoke(builderObj, x, y, w, h);
            Object btn = build.invoke(bounded);
            addWidget.invoke(screen, btn);
            added++;
        }

        try {
            Method reposition = screenCls.getMethod("repositionElements");
            reposition.invoke(screen);
        } catch (NoSuchMethodException ignored) {
        }

        nativeLog("[RsiftUiBridge] added " + added + " button(s) on " + screenClass + " at " + sw + "x" + sh);
    }

    private static native void nativePrepareScreen(String screenClass);
    private static native int nativeButtonCount(String screenClass);
    private static native int nativeButtonId(String screenClass, int index);
    private static native String nativeButtonLabel(String screenClass, int index);
    private static native int nativeButtonX(String screenClass, int index);
    private static native int nativeButtonY(String screenClass, int index);
    private static native int nativeButtonW(String screenClass, int index);
    private static native int nativeButtonH(String screenClass, int index);
    private static native void nativeLog(String line);
}
