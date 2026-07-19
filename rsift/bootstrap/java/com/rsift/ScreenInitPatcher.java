package com.rsift;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;

/**
 * Patches {@code Screen.init} / {@code rebuildWidgets} to call
 * {@link RsiftScreenHooks#onScreenInit(Object)} before each return.
 * Uses COMPUTE_MAXS only — COMPUTE_FRAMES during transform can crash the JVM (0xC0000005).
 */
public final class ScreenInitPatcher {
    private static final String HOOK = "com/rsift/RsiftScreenHooks";
    private static final String HOOK_METHOD = "onScreenInit";
    private static final String HOOK_DESC = "(Ljava/lang/Object;)V";

    private ScreenInitPatcher() {}

    public static byte[] patch(byte[] input) {
        try {
            ClassReader reader = new ClassReader(input);
            ClassWriter writer = new ClassWriter(reader, ClassWriter.COMPUTE_MAXS);
            ClassVisitor visitor = new ClassVisitor(Opcodes.ASM9, writer) {
                @Override
                public MethodVisitor visitMethod(
                        int access,
                        String name,
                        String descriptor,
                        String signature,
                        String[] exceptions) {
                    MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                    if (!isHookTarget(name, descriptor)) {
                        return mv;
                    }
                    return new MethodVisitor(Opcodes.ASM9, mv) {
                        @Override
                        public void visitInsn(int opcode) {
                            if (opcode == Opcodes.RETURN) {
                                mv.visitVarInsn(Opcodes.ALOAD, 0);
                                mv.visitMethodInsn(
                                        Opcodes.INVOKESTATIC,
                                        HOOK,
                                        HOOK_METHOD,
                                        HOOK_DESC,
                                        false);
                            }
                            super.visitInsn(opcode);
                        }
                    };
                }
            };
            reader.accept(visitor, ClassReader.EXPAND_FRAMES);
            return writer.toByteArray();
        } catch (Throwable t) {
            return input;
        }
    }

    private static boolean isHookTarget(String name, String descriptor) {
        if (!"init".equals(name) && !"rebuildWidgets".equals(name)) {
            return false;
        }
        return "()V".equals(descriptor) || "(II)V".equals(descriptor);
    }
}
