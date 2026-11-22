# Más widgets, más tarjetas de la isla y el «doble escritorio»

## 1. Doble escritorio: uno para los widgets y otro para los iconos

**Sí es factible.** Hay tres formas; propongo la A como base y la B como opción.

### A. Dos capas en el mismo escritorio (recomendada)
El escritorio tiene dos «caras» y se pasa de una a otra con una transición:
- **Cara de widgets**: iconos ocultos, widgets visibles (lo que haces ahora a mano).
- **Cara de iconos**: los widgets se apartan y aparecen los iconos.

**Cómo se cambia:**
- Doble clic en un hueco vacío del escritorio (como Fences).
- Deslizar con tres dedos o la rueda lateral sobre el escritorio.
- Botón en la barra (icono del escritorio) o atajo `Win+Alt+D`.

**Animación:** los widgets se deslizan y se desvanecen hacia un lado (con un ligero desenfoque y escala 0,96), y los iconos entran desde el otro con un fundido. Al volver, al revés. Un indicador de dos puntos, como los de los escritorios virtuales, aparece un momento en la isla.

**Técnica:** mostrar u ocultar los iconos es lo que hace Windows con *Ver → Mostrar iconos del escritorio* (mensaje 0x7402 a la vista del escritorio). Los widgets ya se pueden mostrar u ocultar (`desk::set_hidden`). No toca archivos ni el registro más allá de esa opción. Dificultad: **baja-media**; lo delicado es que la transición no parpadee porque Explorer repinta los iconos de golpe.

### B. Ligado a los escritorios virtuales de Windows (opción)
«Escritorio 1 = widgets, Escritorio 2 = iconos» (o el reparto que elijas). Al cambiar con `Win+Ctrl+←/→` o con cuatro dedos, Windows ya hace su animación de deslizar y macbar enseña u oculta iconos y widgets según el escritorio. Se parece mucho a Linux.
- macbar ya detecta el cambio de escritorio virtual (es el aviso de los puntos en la isla).
- Pega: los iconos del escritorio son los mismos en todos los escritorios virtuales, así que hay que conmutarlos en cada cambio. Puede verse medio segundo de iconos al llegar; habrá que medirlo.
- Dificultad: **media**.

### C. Carpeta del escritorio dentro de un widget (complemento)
Un widget «Escritorio» que enseña los iconos de tu carpeta Escritorio en una rejilla (como un portal de Fences), con abrir, arrastrar y menú contextual. Así nunca hace falta enseñar los iconos sueltos. Dificultad: **media** (rejilla, miniaturas y arrastrar ya existen en la bandeja).

---

## 2. Widgets nuevos (útiles y bonitos)

| # | Widget | Qué enseña | Lo bonito | Dificultad |
|---|---|---|---|---|
| 1 | **Visualizador de audio** | El espectro de lo que suena en el PC | Barras o una onda suave con el color de la carátula; se duerme en silencio | Media (WASAPI en bucle + FFT) |
| 2 | **Marco de fotos** | Fotos de una carpeta que elijas, rotando | Efecto Ken Burns lento y fundido entre fotos; esquinas redondeadas | Baja |
| 3 | **Relojes del mundo** | 2-4 ciudades con hora, día/noche y diferencia | Esferas analógicas pequeñas con el cielo según la hora | Baja |
| 4 | **Dispositivos y baterías** | Auriculares, ratón, teclado y mando Bluetooth con su batería | Anillos de batería con el icono de cada aparato | Media (Bluetooth LE: servicio de batería) |
| 5 | **Pomodoro y concentración** | Bloques de hoy y de la semana | Gráfica de barras de la semana con racha | Baja (los datos ya existen) |
| 6 | **Uso de Claude** | Uso semanal y cuándo se reinicia | Anillo grande con cuenta atrás al reinicio | Baja (el dato ya está) |
| 7 | **Lista de tareas** | Tareas locales con casillas | Tachado animado al completar; las hechas se van con un fundido | Baja |
| 8 | **Cuenta atrás** | Días que faltan para fechas que elijas | Número grande que rueda cada día | Baja |
| 9 | **Lanzador** | Tus apps y carpetas favoritas en una rejilla | Iconos que crecen un poco al pasar el ratón, como un dock | Baja |
| 10 | **Red** | Velocidad ahora, IP, Wi-Fi, ping | Gráfica viva de subida y bajada | Baja (parte ya está en Sistema) |
| 11 | **Luna y sol** | Fase lunar, salida y puesta del sol | Arco del sol del día con la posición actual | Baja |
| 12 | **Portapapeles** | Lo último copiado, clic para volver a copiarlo | Tarjetas apiladas que se abren en abanico | Baja |

---

## 3. Más tarjetas de la isla (notch extendido)

Situaciones en las que la isla se despliega sola, o al hacer clic, con una tarjeta propia:

| # | Tarjeta | Cuándo | Qué trae | Dificultad |
|---|---|---|---|---|
| 1 | **Salida de audio** | Rueda larga o clic derecho en la isla de volumen | Lista de salidas (altavoces, auriculares, HDMI) para cambiar con un clic y volumen por app | Media (cambiar la salida por defecto usa una interfaz no documentada de Windows) |
| 2 | **Llamada en curso** | Teams, Zoom, Meet o Discord usando micro | Tiempo de llamada, silenciar micro, apagar cámara, volver a la app | Media |
| 3 | **Auriculares conectados** | Al conectar unos Bluetooth | Tarjeta tipo AirPods con nombre, icono y batería | Media |
| 4 | **Descarga en curso** | Un archivo creciendo en Descargas | Nombre, progreso y velocidad; al acabar se convierte en el aviso de descarga | Media |
| 5 | **Cargando** | Al enchufar | Batería grande, tiempo hasta llenarse y modo de energía | Baja |
| 6 | **Grabación de pantalla** | Recortes u OBS grabando | Punto rojo con el tiempo y botón de parar (si la app lo permite) | Media |
| 7 | **Portapapeles** | Pestaña nueva de la tarjeta | Los últimos 6 copiados para volver a copiarlos | Baja |
| 8 | **Notificación del móvil** | Al llegar una | El mensaje entero, la app y el botón de abrirlo en Enlace Móvil | Baja |
| 9 | **Bandeja de capturas** | Tras una captura | Miniatura grande con copiar, guardar, anotar y arrastrar | Baja |

---

## 4. Orden propuesto

1. **Doble escritorio A** (con su transición y el doble clic). Es lo que pides y es lo que más se nota.
2. **Visualizador de audio** y **marco de fotos**: los más vistosos.
3. **Tarjeta de salida de audio** y **tarjeta de llamada**: las más útiles en el día a día.
4. **Dispositivos y baterías** y **auriculares conectados** (comparten la parte de Bluetooth).
5. Opción **B** (ligado a los escritorios virtuales) y widget **Carpeta del escritorio (C)**.
6. El resto de widgets pequeños (relojes, tareas, cuenta atrás, lanzador, luna, portapapeles).

Cada paso con sus tests, capturas en modo demo y su propio commit.
