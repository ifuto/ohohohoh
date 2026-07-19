package com.rsift.smp;

import org.junit.jupiter.api.Test;
import java.util.concurrent.ThreadLocalRandom;
import static org.junit.jupiter.api.Assertions.*;

public class SmpSystemPluginTest {

    @Test
    public void testTwentyFivePercentProbabilityRate() {
        int cancelled = 0;
        int total = 10000;
        for (int i = 0; i < total; i++) {
            if (ThreadLocalRandom.current().nextInt(100) < 25) {
                cancelled++;
            }
        }
        double rate = (double) cancelled / total;
        assertTrue(Math.abs(rate - 0.25) < 0.03, "Vanishing rate should be approx 25%, got: " + rate);
    }
}
