/**
 * RSIFT OFFICIAL HOMEPAGE — INTERACTIVE SCRIPT ENGINE
 * Handles Benchmark Tab Switching, Chart Animation, and Terminal Simulation.
 */

document.addEventListener('DOMContentLoaded', () => {
  initBenchmarkTabs();
  initTerminalSimulation();
});

/**
 * Benchmark Showcase Tabs Controller
 */
function initBenchmarkTabs() {
  const tabBtns = document.querySelectorAll('.tab-btn');
  const tabContents = document.querySelectorAll('.tab-content');

  tabBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      const targetId = btn.getAttribute('data-tab');

      // Remove active from all buttons & contents
      tabBtns.forEach(b => b.classList.remove('active'));
      tabContents.forEach(c => c.classList.remove('active'));

      // Add active to clicked button & targeted content
      btn.classList.add('active');
      const targetContent = document.getElementById(targetId);
      if (targetContent) {
        targetContent.classList.add('active');
        animateBars(targetContent);
      }
    });
  });

  // Animate initial active tab
  const activeContent = document.querySelector('.tab-content.active');
  if (activeContent) {
    animateBars(activeContent);
  }
}

/**
 * Animates chart bars when a tab becomes active
 */
function animateBars(container) {
  const bars = container.querySelectorAll('.bar-fill');
  bars.forEach(bar => {
    const targetWidth = bar.getAttribute('data-width') || '100%';
    bar.style.width = '0%';
    setTimeout(() => {
      bar.style.width = targetWidth;
    }, 50);
  });
}

/**
 * Terminal Simulation - Loops lifelike boot sequence logs
 */
function initTerminalSimulation() {
  const terminalBody = document.getElementById('terminal-logs');
  if (!terminalBody) return;

  const logs = [
    '<span class="cmd">$ rsift.exe --mod-dir ./mods --min-heap 4G --max-heap 8G</span>',
    '<span class="dim">[00:00:00.001] [INFO]</span> Rsift Native Injection Host v1.0.0 initializing...',
    '<span class="dim">[00:00:00.003] [INFO]</span> CPU Topology: 8 physical P-Cores, AVX2 / FMA3 / BMI2 enabled',
    '<span class="dim">[00:00:00.004] [INFO]</span> Invoking JNI_CreateJavaVM for Minecraft 1.21.11 child process...',
    '<span class="ok">[00:00:00.011] [SUCCESS]</span> JVM attached. Host process retaining memory priority.',
    '<span class="dim">[00:00:00.012] [INFO]</span> JVMTI ClassFileLoadHook active (AVX2 parallel scanner).',
    '<span class="dim">[00:00:00.014] [INFO]</span> Discovering native shared libraries (.dll) in ./mods...',
    '<span class="hl">[00:00:00.015] [MOD]</span> Loaded <span class="ok">rsgraphics.dll</span> (Next-Gen GPU-Driven Rendering Engine)',
    '<span class="hl">[00:00:00.016] [MOD]</span> Loaded <span class="ok">rscalc.dll</span> (AOT Transpiled Physics & AI Simulation Engine)',
    '<span class="hl">[00:00:00.017] [MOD]</span> Loaded <span class="ok">rsreplay.dll</span> (Frame-Locked Synchronous MP4 Capture)',
    '<span class="ok">[00:00:00.019] [SUCCESS]</span> All 3 native plugins verified (`bytemuck::Pod` zero-copy IPC bridge active).',
    '<span class="cmd">[00:00:00.021] [RSGRAPHICS]</span> Intercepting LWJGL OpenGL/Vulkan endpoints -> redirecting to wgpu/DX12...',
    '<span class="ok">[00:00:00.025] [RSGRAPHICS]</span> Bindless Descriptor Ring & ChunkBucketGate initialized.',
    '<span class="cmd">[00:00:00.028] [READY]</span> Minecraft 1.21.11 running at bare-metal native velocity. Enjoy!'
  ];

  let index = 0;
  terminalBody.innerHTML = '';

  function appendNextLog() {
    if (index < logs.length) {
      const line = document.createElement('div');
      line.innerHTML = logs[index];
      line.style.opacity = '0';
      line.style.transition = 'opacity 0.25s ease';
      terminalBody.appendChild(line);
      
      setTimeout(() => { line.style.opacity = '1'; }, 20);
      terminalBody.scrollTop = terminalBody.scrollHeight;
      
      index++;
      setTimeout(appendNextLog, index < 4 ? 120 : (index === 7 ? 350 : 200));
    } else {
      // Loop sequence every 12 seconds
      setTimeout(() => {
        index = 0;
        terminalBody.innerHTML = '';
        appendNextLog();
      }, 12000);
    }
  }

  appendNextLog();
}
