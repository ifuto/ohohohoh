package com.rsift;

import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.network.chat.Component;

public final class RsiftScreenInjector {
    private RsiftScreenInjector() {}

    public static void injectButtons(Screen screen, String screenClass) {
        nativePrepareScreen(screenClass);
        int count = nativeButtonCount(screenClass);
        for (int i = 0; i < count; i++) {
            int id = nativeButtonId(screenClass, i);
            String label = nativeButtonLabel(screenClass, i);
            int x = nativeButtonX(screenClass, i);
            int y = nativeButtonY(screenClass, i);
            int w = nativeButtonW(screenClass, i);
            int h = nativeButtonH(screenClass, i);
            Button btn = Button.builder(Component.literal(label), b -> nativeOnButton(id))
                    .bounds(x, y, w, h)
                    .build();
            screen.addRenderableWidget(btn);
        }
    }

    public static void onClientTick() {
        nativeClientTick();
    }

    private static native void nativePrepareScreen(String screenClass);
    private static native int nativeButtonCount(String screenClass);
    private static native int nativeButtonId(String screenClass, int index);
    private static native String nativeButtonLabel(String screenClass, int index);
    private static native int nativeButtonX(String screenClass, int index);
    private static native int nativeButtonY(String screenClass, int index);
    private static native int nativeButtonW(String screenClass, int index);
    private static native int nativeButtonH(String screenClass, int index);
    private static native void nativeOnButton(int buttonId);
    private static native void nativeClientTick();
}
