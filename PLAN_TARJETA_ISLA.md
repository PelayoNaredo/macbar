# Tarjeta de la isla: plan completo

Objetivo: que la isla, además de ensancharse dentro de la barra, pueda **crecer hacia abajo y convertirse en una tarjeta grande** con controles de verdad (como la Dynamic Island expandida del iPhone o NotchNook en Mac), con el mismo estilo limpio de macbar.

---

## 1. Qué es y qué no es

- **Es** una extensión de la isla: nace de la píldora negra y vuelve a ella. Nunca aparece «de la nada» como un panel más.
- **No es** otro panel del tipo centro de control: no tiene flecha, ni fondo acrílico, ni borde de panel. Es negra como la isla, con el borde fino de la isla.
- **No sustituye** a los avisos (HUD) ni al modo compacto ni al desplegado que ya existen: se suma por encima.

---

## 2. Arquitectura

### 2.1 Ventana propia
- Ventana nueva `card` (módulo `src/card.rs`): ventana capa, sin activar al aparecer, siempre encima, fuera de la barra de tareas, pintada con Direct2D como los paneles (mismo `RtKey` para las cachés).
- **Cubre la isla y la tarjeta**: su rectángulo va desde el borde superior de la isla hasta el borde inferior de la tarjeta (o al revés con la barra abajo), con el ancho máximo de la tarjeta más el margen de la sombra.

### 2.2 El relevo (lo más delicado)
1. Al pedir la tarjeta, la barra le pasa el **rectángulo actual de la isla** (`g.isl_rect`, en píxeles de pantalla) y el **contenido que mostraba**.
2. La ventana de la tarjeta pinta su primer fotograma **con la forma y el contenido exactos de la isla en ese instante**.
3. **Solo después** de que ese fotograma esté en pantalla, la barra deja de pintar la isla (bandera `CARD_OWNS_ISLAND`). Así nunca se ven dos islas ni un parpadeo.
4. Al cerrar se hace lo mismo a la inversa: la tarjeta se encoge hasta el rectángulo de la isla, la barra vuelve a pintarla y, en el siguiente fotograma, la tarjeta se oculta.
5. Si la isla ha cambiado de ancho mientras la tarjeta estaba abierta (por ejemplo, la canción terminó), la tarjeta se encoge hacia el **rectángulo nuevo**, que se le pide a la barra en cada fotograma del cierre.

### 2.3 Quién decide
- `card::open(tab, origen)`, `card::close(motivo)`, `card::is_open()` y `card::owns_island()`.
- La decisión de cerrar vive en una **función pura** `should_close(estado, evento) -> Option<Motivo>` que se prueba con tests (sección 9).

---

## 3. Cuándo se abre

### 3.1 Disparadores
| Situación de la isla | Clic izquierdo | Ratón encima (si está activado) | Arrastrar archivos encima |
|---|---|---|---|
| Música (compacta o desplegada) | Abre la tarjeta en **Música** | Se abre tras **450 ms** quieto encima | Abre en **Bandeja** |
| Temporizador o pomodoro | Abre en **Temporizador** | Igual | Abre en **Bandeja** |
| Bandeja | Abre en **Bandeja** | Igual | Abre en **Bandeja** |
| Aviso (HUD) | **No abre**: hace lo de hoy (cerrar el aviso o abrir su destino) | No abre | Abre en **Bandeja** |
| Sin nada (isla invisible) | Solo si **Vistazo** está activado: clic en la zona central de la barra (120 DIP) | No | Abre en **Bandeja** (vacía, lista para soltar) |

### 3.2 Botones de la isla desplegada
- Los botones que ya existen en la isla desplegada (anterior, pausa y siguiente; parar el temporizador; compartir y vaciar la bandeja) **siguen funcionando igual**. Solo un clic fuera de un botón abre la tarjeta.
- **Cambio de comportamiento:** hoy un clic en la isla compacta de música pausa o reanuda. Con la tarjeta, ese clic **la abre**.
  - Pausar sigue a un gesto: botón central dentro de la tarjeta, clic central sobre la isla, o las teclas multimedia.
  - El ajuste «Clic en la isla» deja elegir: **Abrir la tarjeta** (por defecto) o **Pausar/reanudar** (como hasta ahora; entonces la tarjeta se abre solo pasando el ratón).

### 3.3 Cuándo no se abre nunca
- **Pantalla completa** (`fullscreen()`): no se abre y, si estaba abierta, se cierra.
- **Modo presentación:** solo con clic, nunca con el ratón encima ni al arrastrar.
- **Sesión bloqueada o cambio de usuario:** no se abre.
- **Durante el arranque** (los 450 ms de entrada de la barra): espera a que termine.
- **Arrastrando un elemento de la barra o con un menú nativo abierto**: espera a que se cierre el menú.

### 3.4 Convivencia con otros paneles
- Abrir la tarjeta **cierra** el centro de control, los iconos ocultos, los paneles (Wi-Fi, tiempo, calendario…) y el buscador.
- Abrir cualquiera de ellos **cierra** la tarjeta, con su animación de vuelta a la isla, a la vez que el otro entra.
- Los menús nativos (clic derecho, menú de la app) cierran la tarjeta sin animación de rebote: solo fundido rápido (90 ms), para que no se peleen visualmente.

---

## 4. Cuándo se cierra

| Evento | Abierta con clic | Abierta con el ratón encima |
|---|---|---|
| El ratón sale de isla y tarjeta | Se cierra tras **700 ms** fuera | Se cierra tras **250 ms** fuera |
| El ratón vuelve antes de ese plazo | Se cancela el cierre | Se cancela el cierre |
| Clic fuera | Se cierra al momento | Se cierra al momento |
| Esc | Se cierra | Se cierra |
| Clic en la isla con la tarjeta abierta | Se cierra (alterna) | Se cierra |
| Arrastrando la barra de progreso o un archivo | **Nunca** mientras dure el arrastre; el plazo empieza al soltar | Igual |
| Se acaba lo que mostraba (la música se cierra, termina el temporizador) | Si queda otra pestaña con contenido, cambia a ella; si no, enseña «Nada sonando» 1,5 s y se cierra | Igual |
| App a pantalla completa, bloqueo, presentación activada | Se cierra al momento | Igual |
| Se abre otro panel o menú | Se cierra | Se cierra |
| Cambio de DPI o de monitor, o se recrea la superficie | Se recoloca sin cerrarse (y sin animar) | Igual |
| Barra cambiada de arriba a abajo | Se cierra sin animación | Igual |

- **Nunca se cierra sola por tiempo** si el ratón sigue encima.
- El cierre se puede **interrumpir**: si el ratón vuelve a mitad del encogimiento, la tarjeta vuelve a crecer desde el tamaño que tenga en ese momento (los muelles parten del valor actual, sin saltos).

---

## 5. Contenido

### 5.1 Pestañas
- Pestañas en forma de píldoras pequeñas arriba a la derecha (iconos, sin texto). **Solo aparecen si hay más de una con contenido.**
- Orden fijo: **Música · Temporizador · Bandeja · Vistazo**.
- **Pestaña inicial:** la del modo que enseñaba la isla al abrirla. Si se abre al arrastrar archivos, Bandeja. Si no hay nada, Vistazo.
- Cambiar de pestaña: clic, rueda del ratón sobre las pestañas, o Tab y Mayús+Tab con la tarjeta enfocada. El contenido se desliza 12 DIP hacia el lado de la pestaña nueva con fundido cruzado (200 ms) y el alto de la tarjeta se anima al de la nueva.
- Se recuerda la última pestaña elegida **solo mientras dure la sesión**.

### 5.2 Música (380 × 168 DIP)
- **Carátula** de 76 DIP con esquinas de 14. Hace falta subir `ART_PX` de 96 a **256** (a 200 % se ve borrosa a 96). Se reduce para la isla con la interpolación de alta calidad de Direct2D.
- **Título** en dos líneas como mucho, con puntos suspensivos. Debajo, artista · álbum en gris; si no caben, solo el artista.
- **Fuente**: icono de la app (YouTube Music, Spotify…) en pequeño en la esquina de la carátula; clic = traer el reproductor al frente.
- **Barra de progreso:**
  - Con tiempo transcurrido a la izquierda y lo que queda a la derecha («-1:23»).
  - Engorda de 4 a 8 DIP al pasar el ratón, y el círculo de arrastre aparece con un pop.
  - **Arrastrable** solo si hay línea de tiempo (`TIMELINE`) y la app deja saltar: el primer `Seek` fallido la marca como no arrastrable durante esa canción y la barra se queda de solo lectura (sin círculo).
  - Mientras se arrastra, el tiempo muestra la posición de destino y la música no salta hasta soltar.
  - Si cambia la canción a mitad del arrastre, este se cancela.
  - Sin línea de tiempo, no hay barra: el hueco lo ocupa el ecualizador animado.
- **Controles**: anterior · pausa · siguiente de 34 DIP, más **-15 s / +15 s** solo si se puede saltar. Los que la app no permite van atenuados al 25 % y no responden.
- **Fondo**: degradado del color de la carátula (`tint_now`) al 35 % arriba, hasta negro abajo; se funde al color nuevo en 350 ms al cambiar de canción.
- **Al cambiar de canción**:
  - La carátula nueva entra con el pop que ya tiene la isla.
  - El título y el artista hacen fundido cruzado con un desplazamiento de 6 DIP.
  - La barra de progreso vuelve a 0 deslizándose, no de golpe.
- **En pausa**: la carátula se encoge al 92 % con un muelle suave (como en el iPhone) y el ecualizador se para.

### 5.3 Temporizador y pomodoro (340 × 150 DIP)
- **Anillo** grande de 72 DIP con la cuenta dentro en grande (cifras que ruedan, como en la isla).
- **Color**:
  - Naranja normalmente.
  - Verde en los descansos.
  - Rojo en los últimos 10 s de un temporizador normal.
- **Botones**:
  - **+1 min**.
  - **Pausa/reanudar**: hay que añadir la pausa a `live` (guardar lo que queda en vez del instante final).
  - **Parar**.
- **Pomodoro**:
  - Fase («Concentración» o «Descanso») y los cuatro puntos.
  - Botón **Saltar fase**.
  - Contador de bloques del día.
- **Sin temporizador** (pestaña abierta desde Vistazo): chips de 1, 5, 10 y 25 min y Pomodoro, para arrancar con un clic.
- **Al terminar con la tarjeta abierta**:
  - El anillo se llena entero, se sacude como la isla y aparece **Repetir**.
  - Si el ratón no está encima, se cierra pasados 6 s.

### 5.4 Bandeja (380 × alto según archivos, 2 filas como mucho)
- **Rejilla** de 4 columnas: miniatura de 56 DIP con el nombre en una línea debajo.
  - Con más de 8 archivos, la segunda fila se desplaza con la rueda.
- **Acciones sobre un archivo**:
  - Clic: abrirlo.
  - Clic derecho: menú con Mostrar en la carpeta, Compartir y Quitar de la bandeja.
  - **Arrastrar hacia fuera**: lo lleva a otra app (necesita `DoDragDrop` con un objeto de datos con CF_HDROP; es la parte más costosa, va en la fase 3).
- **Botones**: **Compartir todo** y **Vaciar**.
- **Al arrastrar archivos encima**:
  - La tarjeta muestra una zona punteada que respira, con «Suelta para guardar aquí».
  - Al soltar, las miniaturas nuevas entran una tras otra (40 ms entre cada una).
- **Vacía**: el texto de la zona punteada y nada más.

### 5.5 Vistazo (opcional, apagado por defecto; 340 × 140 DIP)
- Hora grande con segundos, fecha y el tiempo actual (icono y temperatura).
- Batería y uso semanal de Claude, en dos anillos pequeños.
- Fila con los atajos de cafeína, pomodoro, cuentagotas y captura.
- No hay calendario con eventos hoy, así que **no** se muestran eventos (no inventar datos).

### 5.6 Avisos con la tarjeta abierta
- La isla no se pinta mientras la tarjeta es suya, así que los avisos se ven **dentro de la tarjeta**: una franja arriba de 32 DIP con el mismo icono, texto y barra del aviso, que baja deslizándose y se va a su hora.
- Volumen y brillo actualizan esa franja en vivo en vez de apilarse.
- Batería baja, «se acabó el tiempo» y modo presentación **sí** cierran la tarjeta, para que el aviso se vea en la isla como siempre.

---

## 6. Animaciones (tiempos exactos)

### Apertura
- **Ancho y alto:** dos muelles, k = 380 y amortiguación 0,72 (el mismo rebote que la isla). **El alto arranca 30 ms después que el ancho**, para que primero se ensanche y luego caiga, como la Dynamic Island.
- **Radio de esquina:** pasa de la mitad del alto (píldora) a 24 DIP a la vez que crece la altura.
- **Contenido de la isla:** se desvanece en los primeros 90 ms.
- **Contenido de la tarjeta:** entra a partir del 55 % del recorrido, con fundido y 6 DIP hacia arriba.
  - Va por filas, con 25 ms de retraso entre ellas: carátula, título, barra y controles.
  - La carátula hace el viaje de verdad: crece desde su sitio en la isla (22 DIP) hasta su sitio en la tarjeta (76 DIP) como un solo objeto (*shared element*). Sin esto, se nota el corte.
- **Sombra:** suave (desenfoque de 24 DIP, al 45 %). Crece de 0 a su tamaño a la vez que la altura; en tamaño píldora no hay sombra.

### Cierre
- Primero el contenido se desvanece (90 ms) y la carátula viaja de vuelta a la isla.
- Después la forma se encoge con los mismos muelles, esta vez el alto antes que el ancho.

### Detalles finos
- **Botones:**
  - Al pulsar, se encogen al 88 % con un muelle.
  - Al pasar el ratón, un círculo blanco al 10 % que aparece en 120 ms.
- **Pausa y reanudar:** el icono hace morfosis (fundido y escala, como los iconos de la barra).
- **Pestañas:** la píldora de la pestaña activa se desliza hasta la nueva (como los puntos de escritorios).

### Reducir animaciones
- Si Windows tiene desactivadas las animaciones (`SPI_GETCLIENTAREAANIMATION`), todo se reduce a fundidos de 120 ms, sin muelles ni desplazamientos.

### Rendimiento
- Solo se pinta mientras algo se mueve.
- Con todo quieto, un repintado por segundo (progreso o cuenta atrás); en pausa y sin temporizador, ninguno.
- Objetivo: **0 % de CPU** con la tarjeta quieta y en pausa.

---

## 7. Geometría y posición

- **Centrada bajo la isla.** La isla está centrada en la barra, así que la tarjeta también. Si no cabe en el monitor (pantallas muy estrechas), se encaja con 8 DIP de margen.
- **Hueco:** la tarjeta *continúa* la isla. Su borde superior coincide con el de la isla, sin hueco y sin solaparse con la barra visualmente; la barra no se oscurece.
- **Barra abajo:** crece hacia arriba (todo se espeja en vertical, salvo el contenido). Las pestañas pasan abajo a la derecha.
- **Monitor de la barra, con su DPI.** Al cambiar el DPI se recalcula todo.
- **Alto máximo:** el área de trabajo menos 16 DIP.

---

## 8. Teclado y accesibilidad

- La tarjeta **se activa al abrirla con clic** (para recibir el teclado) y devuelve el foco a la ventana anterior al cerrarse, como los paneles (`return_to`). Abierta con el ratón encima, **no roba el foco**.
- **Teclas con la tarjeta enfocada:**
  - Espacio: pausa o reanuda.
  - ← / →: salta 5 s si se puede.
  - ↑ / ↓: volumen.
  - Tab: siguiente pestaña.
  - Esc: cierra.
- **Lectores de pantalla:** nombre de ventana «Isla de macbar» y descripción con lo que suena. No se exige más en esta versión.

---

## 9. Pruebas

### Pruebas unitarias (funciones puras)
- `card_size(tab, n_archivos, escala)`: tamaños y alto máximo.
- `initial_tab(modo_isla, origen, pestañas_con_contenido)`.
- `should_close(...)`: toda la tabla de la sección 4, una prueba por fila, incluidas las de arrastre en curso y vuelta del ratón dentro del plazo.
- `place(isla, tamaño, monitor, abajo)`: encaje en el monitor y espejo con la barra abajo.
- `live`: pausa y reanudar del temporizador, +1 min y saltar fase del pomodoro.

### Prueba visual con `MACBAR_CARDTEST=1`
- Abre la tarjeta en cada pestaña, cambia de canción ficticia, simula un aviso de volumen dentro y la cierra.
- Se comprueba con capturas a mitad de animación (al 30 %, 60 % y 100 %) y en reposo, arriba y abajo, al 100 % y al 150 %.

### Comprobaciones a mano (cuando el usuario no esté usando el PC)
- Arrastrar la barra de progreso.
- Soltar archivos.
- Esc y clic fuera.
- Abrir el centro de control con la tarjeta abierta.
- Pantalla completa.

---

## 10. Ajustes nuevos

- **Clic en la isla:** Abrir la tarjeta (por defecto) / Pausar o reanudar.
- **Abrir al pasar el ratón:** sí / no (por defecto **no**, para que no salte sin querer).
- **Vistazo sin nada sonando:** sí / no (por defecto no).
- Se guardan en la configuración (`tarjeta_clic`, `tarjeta_hover`, `tarjeta_vistazo`) y se exportan con los perfiles.

---

## 11. Fases

1. **Ventana, relevo y morfosis + Música.** Incluye subir `ART_PX`, la barra de progreso de solo lectura y la tabla de cierre completa con sus tests. *Aquí se decide si la transición queda fina; si no, no se sigue.*
2. **Barra de progreso arrastrable, ±15 s y teclado.**
3. **Temporizador**: pausa en `live`, +1 min, saltar fase y chips.
4. **Bandeja**: rejilla, soltar con zona punteada, menú, compartir; arrastrar hacia fuera al final de esta fase.
5. **Avisos dentro de la tarjeta, Vistazo y ajustes.**
6. **Pulido**: reducir animaciones, barra abajo, 150 %, CPU en reposo, capturas finales y README.

Cada fase termina compilando, con los tests en verde, comprobada con capturas y en su propio commit local.

---

## 12. Riesgos

- **Parpadeo en el relevo isla ↔ tarjeta** (dos ventanas). Mitigación: la regla del primer fotograma (2.2). Si aun así se nota, plan B: agrandar temporalmente la ventana de la barra hacia abajo y dibujar la tarjeta en ella. Funciona, pero complica el reservado de espacio del appbar, y por eso no es el plan A.
- **Saltos en la música que no todas las apps admiten**: se detecta con el primer fallo (5.2).
- **Arrastrar archivos hacia fuera** es la pieza más frágil (OLE en el hilo de la interfaz); va al final y, si da guerra, se queda en «Mostrar en la carpeta».
- **Choque con el clic actual (pausar)**: resuelto con el ajuste de 3.2. Hay que contarlo en las notas de la versión.
