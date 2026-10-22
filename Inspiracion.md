Construye un prototipo de DAW en Rust con estética cyberpunk inspirado en Ableton Live 2035:

1. ARQUITECTURA:
   - Divide la aplicación en dos hilos:
     a) Hilo de Audio: Usa `cpal` para emitir un stream estéreo de 48kHz / 32-bit float. Implementa un sintetizador senoidal/FM básico y un bus de mezcla estéreo.
     b) Hilo UI: Usa `eframe` (`egui`) con backend `wgpu`.
   - Comunica ambos hilos mediante buffers lock-free usando la librería `rtrb`.

2. INTERFAZ (UI):
   - Tema: Diseña un tema personalizado en egui con paleta oscura (#070b12), acentos en Cyan neón (#00e5ff), Magenta (#e024c3) y bordes sutiles.
   - Barra Superior: Contador de compás, selector de BPM numérico interactivo, botones Play/Stop y medidor de CPU.
   - Panel Superior Dividido:
     * Izquierda: Matriz de canales con slots de clips simulados y faders verticales con medidores de dB.
     * Derecha: Visor de pistas horizontales y una sección dedicada para renderizar una malla FFT básica usando wgpu o egui Painter.
   - Panel Inferior (Device Chain):
     * Contenedor horizontal modular con al menos 2 cajas de efectos simuladas con perillas rotatorias de Ganancia/Corte que afecten los parámetros del sintetizador en tiempo real.