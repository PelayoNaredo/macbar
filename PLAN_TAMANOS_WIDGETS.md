# Plan: widgets con tamaños y contenido según el tamaño + configuración de Bolsa y Noticias

## 1. Decisión: tamaños fijos, no redimensionado libre

Con redimensionado libre, cada widget tendría que verse bien en cualquier proporción, y eso no se puede pulir. Con **7 tamaños fijos** sobre una rejilla, cada combinación de widget y tamaño se diseña a mano, como en macOS e iOS.

Las medidas van en una rejilla de **celda = 170 DIP** con **20 de hueco**:

| Tamaño | Forma | Medidas (DIP) | Celdas |
|---|---|---|---|
| Pequeño | cuadrado | 170 × 170 | 1×1 |
| Mediano | horizontal | 360 × 170 | 2×1 |
| Banda | horizontal fino | 740 × 170 | 4×1 |
| Alto | vertical | 170 × 360 | 1×2 |
| Grande | cuadrado | 360 × 360 | 2×2 |
| Columna | vertical alto | 360 × 550 | 2×3 |
| Extragrande | horizontal | 740 × 360 | 4×2 |

Reglas:
- **Tamaños permitidos:** cada widget (y cada diseño alternativo) declara cuáles admite. Si un tamaño no tiene sentido, no se ofrece. Por ejemplo, Móvil no admite Banda.
- **Elegir tamaño:** desde el menú «Tamaño» del clic derecho, desde Configuración o arrastrando la esquina inferior derecha.
- **Arrastre:** al pasar el ratón por la esquina aparece un asa; al arrastrar, un contorno fantasma salta al tamaño permitido más cercano, y al soltar el widget se transforma con un muelle. Con «Reducir movimiento» cambia sin animación.
- **Contenido:** al cambiar de tamaño el widget recoloca su contenido y añade o quita bloques; no se limita a escalar. La tipografía, los márgenes y los radios son los mismos en todos los tamaños.
- **Posición:** al cambiar de tamaño se mantiene la esquina superior izquierda. Si el widget se sale de la pantalla se mete dentro, y si pisa a otro widget se desplaza al hueco libre más cercano. Nunca se solapan.
- **Guardado:** se guarda en `widgets.ini` como `tam_<clave> = mediano`. Quien venga de v0.5 conserva su tamaño actual, mapeado al tamaño fijo más cercano.
- **Varios monitores y DPI:** las medidas son DIP y se escalan con el DPI del monitor de cada widget.
- **Cristal:** el mapa de refracción se rehace al cambiar de tamaño; ya está preparado para eso.

## 2. Contenido de cada widget según el tamaño

| Widget | Pequeño | Mediano | Banda | Alto | Grande | Columna | Extragrande |
|---|---|---|---|---|---|---|---|
| Tiempo | temp + icono + ciudad | + máx/mín y 4 horas | + 12 horas en línea | temp arriba + 5 días | + horas y 5 días | + 7 días + detalles (viento, UV, lluvia) | horas + 7 días + detalles |
| Calendario | día grande + día de la semana | día + 3 próximos eventos | semana en línea con eventos | mes mini + 2 eventos | mes completo + eventos de hoy | mes + agenda de 7 días | mes + agenda + semana |
| Música | carátula + play | carátula + título + controles | + progreso y cola | carátula grande + controles | carátula + progreso + controles | + letra/cola | carátula + progreso + espectro |
| Sistema | anillo de CPU | 3 anillos (CPU, RAM, disco) | anillos + red | 3 barras verticales | anillos + red + batería | + procesos que más gastan | todo + gráfica de 60 s |
| Almacenamiento | unidad principal (anillo) | 2 unidades | todas en línea | todas en columna | todas + tamaño | + carpetas más grandes | todas + carpetas |
| Proyectos | último proyecto | 3 recientes | 4 en línea | 5 recientes | recientes + rama/cambios | + commits recientes | dos columnas |
| Móvil | batería + conexión | foto del móvil + estado + nº de avisos | — | foto + estado + accesos | + fotos recientes | + 6 fotos | — |
| Fotos | foto | foto horizontal | tira de 4 | foto vertical | foto grande | 2 fotos | collage |
| Visualizador | espectro mini | espectro | espectro ancho | espectro vertical | espectro + título | + carátula | espectro grande + título |
| Relojes | 1 reloj | 2 relojes | 4 relojes | 3 en columna | 4 en rejilla | 6 | 8 + diferencia horaria |
| Tareas | nº pendientes + 1 tarea | 3 tareas | 4 en línea | 5 tareas | 7 + añadir | 12 + añadir | dos columnas |
| Cuenta atrás | días | días + nombre | varias en línea | 2 cuentas | 3 cuentas | 5 cuentas | 4 en rejilla |
| Noticias | 1 titular | 2 titulares | 3 titulares en línea | 4 titulares | 4 + fuente | 7 titulares | 2 columnas de 4 |
| Bolsa | 1 valor + gráfica | 2 valores | 4 valores en línea | 4 valores | 5 + gráfica grande del primero | 8 valores | 4 + gráfica grande + detalle |

Los diseños alternativos (Cielo, Vinilo, Casete, Ojo…) declaran sus propios tamaños. Si eliges un tamaño que tu diseño no admite, el selector solo enseña los diseños que sí lo admiten, para que no salga nada roto.

## 3. Configuración de Bolsa y Noticias (dentro de Configuración, sin Bloc de notas)

Hace falta un **campo de texto propio** en la ventana de Configuración (con caret, selección, copiar y pegar e IME). Se aprovecha lo que ya hace el editor de Tareas.

**Bolsa**
- Una lista de valores que se puede reordenar arrastrando, con quitar (×) en cada fila.
- **Añadir con buscador:** escribes «santander» y salen sugerencias de Yahoo Finance (nombre, símbolo y mercado). No hace falta saber que es `SAN.MC`.
- **Opciones:**
  - Periodo de la gráfica: día, 5 días, mes o año.
  - Mostrar la variación en % o en €/$.
  - Mostrar u ocultar la gráfica.
  - Frecuencia de refresco.

**Noticias**
- **Temas listos para marcar** (Google Noticias): Portada, España, Mundo, Economía, Tecnología, Ciencia, Salud, Deportes y Entretenimiento.
- **País e idioma** de las noticias.
- **Fuentes propias:** pegas la URL de un RSS y se comprueba al momento si funciona.
- **Filtros:**
  - Palabras a silenciar, para que no salgan titulares que las contengan.
  - Medios a ocultar.
- **Opciones:** cada cuánto pasa de página, si se muestran fuente y hora, y cuántos titulares por página. El máximo lo pone el tamaño.

## 4. Fases

| Fase | Qué | Resultado |
|---|---|---|
| F0 | Base: tamaños, guardado, migración desde v0.5, menú «Tamaño» y selector en Configuración, animación de cambio, colocación sin solapes | Se puede cambiar de tamaño (el contenido aún escalado provisional) |
| F1 | Asa de la esquina con contorno fantasma e imán al tamaño más cercano | Redimensionar arrastrando |
| F2 | Contenido por tamaño, lote 1: Tiempo, Calendario, Música, Sistema | Los 4 más usados, completos |
| F3 | Lote 2: Almacenamiento, Proyectos, Móvil, Fotos, Relojes | |
| F4 | Lote 3: Visualizador, Tareas, Cuenta atrás, Noticias, Bolsa | Todos los clásicos |
| F5 | Diseños alternativos: tamaños que admite cada uno y su reajuste | |
| F6 | Campo de texto + configuración de Bolsa y Noticias | Sin Bloc de notas |
| F7 | Pulido: capturas de todas las combinaciones, tema claro, DPI, varios monitores, reducir movimiento | |

Cada fase acaba con compilación, tests y capturas en modo demo de cada combinación tocada, comparadas con los widgets actuales para que todos sigan pareciendo de la misma familia.
