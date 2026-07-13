# Tech Spec: img2webp-hq

## 1. Назначение

`img2webp-hq` — CLI-утилита для максимально качественного преобразования
одного растрового изображения в статический WebP.

Инструмент ориентирован на:

- корректную обработку цвета;
- корректную обработку alpha;
- high-quality resize;
- сохранение ICC-профиля, когда это возможно и осмысленно;
- прямой контроль над пайплайном кодирования WebP.

Главный приоритет — качество и корректность результата, а не максимальная
простота реализации или минимальная зависимость от внешних библиотек.

---

## 2. Scope v1

В `v1` утилита:

- обрабатывает один входной файл за запуск;
- пишет только статический WebP;
- поддерживает lossy, lossless и near-lossless режимы;
- использует color-managed resize;
- не использует `auto` mode;
- не опирается на shell-вызовы `cwebp` как на основной механизм кодирования.

Вне scope `v1`:

- batch-обработка директорий;
- анимированные форматы;
- полноценная поддержка произвольных не-RGB цветовых моделей;
- generalized image editor.

---

## 3. Поддерживаемые форматы

### 3.1 Входные форматы v1

Обязательные:

- JPEG
- PNG
- WebP

Желательные в следующих этапах:

- TIFF
- AVIF

Отложенные:

- GIF как first-frame-only

### 3.2 Выходной формат

Выходной формат всегда:

- WebP

Поддерживаемые режимы кодирования:

- lossy
- lossless
- near-lossless

### 3.3 Ограничения v1

В `v1` гарантированно поддерживаются:

- RGB/RGBA входы;
- RGB ICC-профили;
- изображения без ICC с предположением `sRGB`.

В `v1` должны завершаться явной ошибкой:

- анимированные входы;
- CMYK, Lab, XYZ, DeviceN и другие не-RGB цветовые модели;
- входы с профилями, для которых не удаётся построить корректный RGB pipeline.

Градации серого допустимы как вспомогательный случай, если декодер может
развернуть их в нейтральный RGB-буфер без неоднозначности. Grayscale с
нестандартным ICC не является целевым кейсом `v1`.

---

## 4. Общие принципы пайплайна

Утилита не должна принудительно сводить всё к `RGBA8` в самом начале.

Пайплайн строится вокруг следующих принципов:

- повышенная внутренняя точность сохраняется как можно дольше;
- `u16` high-quality path остаётся основным режимом по умолчанию;
- expert-флаг `-fast` переключает пайплайн в `RGBA8` internal path;
- resize выполняется только в linear-light;
- alpha обрабатывается в premultiplied-представлении;
- lossy path по умолчанию использует high-bit-depth `simpleyuv`;
- финальная квантовка до 8 бит выполняется максимально поздно;
- выходное изображение не принудительно переводится в `sRGB`, если вход уже
  находится в корректно описанном RGB-пространстве.

---

## 5. Нормализация входа

Перед любыми resize и encode-операциями должны быть выполнены:

1. Декодирование входного изображения.
2. Извлечение ICC, EXIF и XMP при наличии.
3. Применение EXIF Orientation к пикселям.
4. Нормализация внутренних размеров и layout.

### 5.1 EXIF Orientation

Orientation не переносится как есть.

Обязательное поведение:

- orientation применяется к пикселям на этапе decode;
- все последующие этапы работают уже с физически ориентированным изображением;
- если EXIF сохраняется в выходной файл, `Orientation` должен быть сброшен в
  `1` или удалён;
- double-rotation недопустим.

---

## 6. Политика цвета

### 6.1 Базовый контракт

`img2webp-hq` не должен принудительно приводить все входы к `sRGB`.

Если вход содержит корректный RGB ICC-профиль, то:

- этот профиль считается source color space;
- resize выполняется в linearized-версии этого пространства;
- после resize данные возвращаются в нелинейную форму этого же пространства;
- в выходной WebP прикрепляется тот же ICC-профиль, если пользователь не
  отключил его сохранение.

### 6.2 Входы без ICC

Для `v1` при отсутствии ICC используется практическое допущение:

- RGB-вход без ICC трактуется как `sRGB`.

В этом случае:

- resize выполняется в linear `sRGB`;
- если у входа ICC не было, синтезировать новый ICC по умолчанию не требуется;
- выходной WebP может оставаться без ICC, полагаясь на стандартное поведение
  WebP по умолчанию.

### 6.3 CMS

Для color-managed pipeline используется `lcms2`.

`lcms2` отвечает за:

- загрузку входного ICC;
- построение transform'ов между source RGB и рабочим linear-light
  представлением;
- обратный transform перед кодированием;
- валидацию того, что вход действительно попадает в поддерживаемый pipeline.

### 6.4 Что считается ошибкой

Явной ошибкой считаются:

- unsupported profile class;
- unsupported color model;
- невозможность построить корректный transform для рабочего RGB pipeline.

---

## 7. Внутренние представления данных

Предпочтительный порядок внутренней точности:

- сохранять повышенную точность входа, если она доступна;
- использовать `u16` как основной качественный внутренний формат;
- использовать `f32` там, где это упрощает linear-light и premultiplied
  операции;
- выполнять финальную квантовку до 8 бит только непосредственно перед входом в
  WebP encoder path.

### 7.1 JPEG

Для JPEG в `v1` используется специализированный декодер:

- [`rust-jpegli`](https://github.com/bronekot/rust-jpegli)

JPEG decode path должен уметь выдавать `u16` и/или `f32` буферы для более
точной последующей обработки.

Важно:

- higher-precision decode для обычного 8-bit JPEG не "восстанавливает"
  несуществующие старшие биты;
- смысл этого пути — сохранить больше точности для downstream-математики,
  линейзации, premultiply/unpremultiply и финальной квантовки.

### 7.2 PNG и другие high-bit входы

Для high-bit форматов повышенная точность должна сохраняться до последнего
возможного этапа.

Ранняя потеря точности недопустима.

### 7.3 Fast mode

Для практических сценариев, где важнее скорость и более простой код-путь,
поддерживается expert-режим:

- `-fast`

Семантика `-fast`:

- decode выполняется в `RGBA8`, если это возможно;
- при `16-bit` входе ранняя квантовка до `8-bit` допустима;
- CMS и resize также работают в `RGBA8`;
- encode stage получает уже `8-bit` pipeline output;
- это осознанный компромисс в пользу скорости, а не качества.

---

## 8. Resize

### 8.1 Поддерживаемые режимы

Утилита должна поддерживать:

- отсутствие resize;
- resize в точный размер;
- resize с сохранением пропорций;
- fit в ограничение по максимальной ширине/высоте;
- запрет апскейла.

### 8.2 Семантика resize-флагов

Семантика задаётся так:

- только `--width` -> высота вычисляется автоматически с сохранением пропорций;
- только `--height` -> ширина вычисляется автоматически с сохранением пропорций;
- `--width` + `--height` -> точный целевой размер, аспект может измениться;
- `--max-side <N>` -> длинная сторона уменьшается до `N`, вторая вычисляется
  автоматически с сохранением пропорций;
- `--max-width` и/или `--max-height` -> fit внутрь bounding box с сохранением
  пропорций;
- `--width/--height`, `--max-side` и `--max-width/--max-height` нельзя
  смешивать между собой;
- `--no-upscale` запрещает увеличение после вычисления итогового target size.

### 8.3 Фильтры

Поддерживаемые фильтры:

- `lanczos3`
- `catmullrom`
- `mitchell`

Фильтры по умолчанию:

- downscale: `lanczos3`
- upscale: `catmullrom`

### 8.4 Реализация resize

Основной ресайзер:

- `fast_image_resize`

Обязательное требование:

- `fast_image_resize` используется только после перевода изображения в корректное
  linear-light представление;
- resize с alpha выполняется только на premultiplied данных;
- resize напрямую в gamma-coded `sRGB` или gamma-coded source RGB не допускается.

---

## 9. Обработка alpha

Если изображение содержит alpha, утилита обязана использовать alpha-aware path:

1. premultiply alpha;
2. resize;
3. unpremultiply alpha.

Требования:

- полупрозрачные края не должны давать цветные ореолы;
- alpha не должна интерполироваться независимо от premultiplied RGB;
- hidden RGB под полностью прозрачными пикселями не считается приоритетной
  целью сохранения при resize.

### 9.1 Exact transparent RGB

Для совместимости с `cwebp` должен поддерживаться expert-флаг:

- `-exact`

Его смысл:

- просить encoder не выкидывать невидимый RGB там, где это поддерживает
  конкретный encode path.

Но важно:

- если до encode уже был resize или иная операция, меняющая premultiplied
  содержимое, hidden RGB не считается стабильными данными;
- `-exact` не отменяет правила качественного alpha-aware resize.

---

## 10. Режимы кодирования WebP

`img2webp-hq` поддерживает три внутренних режима:

- lossy
- lossless
- near-lossless

`auto` mode в `v1` отсутствует.

CLI должен использовать максимально совместимые с `cwebp` флаги и семантику
там, где это возможно.

### 10.1 Lossy path

Lossy path должен выглядеть так:

1. Decode.
2. Extract metadata.
3. Apply EXIF orientation.
4. Convert to high-precision working buffer.
5. Build color-managed linear working representation, если resize нужен.
6. Premultiply alpha, если alpha присутствует.
7. Resize.
8. Unpremultiply alpha.
9. Convert back from linear to gamma-coded source RGB.
10. Выполнить финальную квантовку как можно позже.
11. Выполнить `RGB -> YUV420` через выбранный lossy path
    (`simpleyuv` по умолчанию, `SharpYUV` при `--sharpyuv`).
12. Encode lossy WebP.
13. Attach metadata according to CLI policy.

Требования:

- lossy path по умолчанию использует high-bit-depth `simpleyuv`;
- ранний переход в `RGBA8` до этапа финальной подготовки цвета недопустим;
- alpha quality должна управляться отдельно;
- chroma conversion и subsampling должны происходить через качественный path.

### 10.2 Что означает SharpYUV в этом проекте

SharpYUV в `img2webp-hq` нужен не для "16-bit WebP".

Он нужен для:

- более аккуратной финальной конверсии high-precision RGB в 8-bit YUV420;
- более качественного chroma downsampling;
- уменьшения артефактов по цветовым границам по сравнению с более грубым
  RGB->YUV path.

Выход WebP lossy всё равно остаётся 8-bit YUV.

### 10.3 Lossless path

Lossless path должен выглядеть так:

1. Decode.
2. Extract metadata.
3. Apply EXIF orientation.
4. Convert to high-precision working buffer.
5. Build linear working representation, если resize нужен.
6. Premultiply alpha, если alpha присутствует.
7. Resize.
8. Unpremultiply alpha.
9. Convert back from linear.
10. Final quantization to `ARGB8`.
11. Encode lossless WebP.
12. Attach metadata according to CLI policy.

Требования:

- lossless path не использует lossy RGB->YUV conversion;
- alpha должна сохраняться корректно;
- ICC должен сохраняться, если это разрешено политикой metadata.

Важно:

- WebP lossless не означает source-lossless для `16-bit` входов;
- это codec-lossless относительно финального `ARGB8` буфера, переданного в
  encoder.

### 10.4 Near-lossless path

Near-lossless path совпадает с lossless path до encode-этапа, после чего
используется near-lossless режим WebP encoder.

Важно:

- near-lossless — отдельный encode mode;
- это не alias для обычного lossy path;
- при его использовании финальный вход в encoder также ограничен `ARGB8`.

---

## 11. Метаданные

### 11.1 Политика CLI

Там, где это возможно, для metadata используются `cwebp`-подобные флаги и
семантика.

Основной флаг:

- `-metadata none|all|exif|icc|xmp`

Значение по умолчанию:

- `icc`

Это означает:

- ICC сохраняется по умолчанию;
- EXIF и XMP по умолчанию не копируются;
- `-metadata all` включает перенос всех поддерживаемых типов metadata.

### 11.2 ICC

Требования:

- если у входа есть ICC и metadata policy включает `icc`, профиль должен быть
  прикреплён к выходному WebP;
- ICC не должен удаляться по умолчанию;
- ICC входа не должен переноситься в выходной файл, если пиксели уже были
  переведены в другое пространство и профиль перестал им соответствовать.

В `v1` этот риск устраняется тем, что RGB-входы не принудительно переводятся в
`sRGB`: resize делается в linearized source RGB и затем данные возвращаются в
source RGB перед encode.

### 11.3 EXIF

Если metadata policy включает `exif`:

- EXIF может быть перенесён;
- `Orientation` внутри EXIF должен быть нормализован и не должен повторно
  переориентировать изображение.

### 11.4 XMP

XMP переносится только если это разрешено metadata policy.

---

## 12. CLI интерфейс

### 12.1 Базовый вызов

```bash
img2webp-hq input.png -o output.webp
```

### 12.2 Принцип именования флагов

Если аналогичный encoder-параметр уже есть в `cwebp`, `img2webp-hq` должен
использовать тот же флаг и максимально близкую семантику.

Собственные флаги добавляются только там, где `cwebp` не покрывает требуемый
high-quality pipeline.

### 12.3 Обязательные encoder-флаги

Совместимые с `cwebp`:

- `-q <float|int>`
- `-alpha_q <int>`
- `-m <0..6>`
- `-sns <int>`
- `-f <int>`
- `--sharpyuv` (alias: `--sharp_yuv`)
- `-lossless`
- `-near_lossless <int>`
- `-exact`
- `-fast`
- `-metadata none|all|exif|icc|xmp`

Поведение:

- если не указан ни `-lossless`, ни `-near_lossless`, используется lossy mode;
- `-lossless` и `-near_lossless` одновременно задавать нельзя;
- `-near_lossless` активирует near-lossless mode;
- `-fast` включает `8-bit` internal pipeline;
- `--sharpyuv` включает lossy path через SharpYUV;
- без `--sharpyuv` lossy path по умолчанию использует high-bit-depth `simpleyuv`.

### 12.4 Resize-флаги

Дополнительные флаги `img2webp-hq`:

- `-o, --output <path>`
- `--width <int>`
- `--height <int>`
- `--max-side <int>`
- `--max-width <int>`
- `--max-height <int>`
- `--no-upscale`
- `--filter lanczos3|catmullrom|mitchell`

---

## 13. Реализация

### 13.1 Язык

- Rust

### 13.2 Основные компоненты

Декодирование:

- `rust-jpegli` для JPEG;
- `image` для PNG/WebP и базовой metadata extraction;
- дополнительные декодеры могут быть добавлены позже для TIFF/AVIF.

Color management:

- `lcms2`

Resize:

- `fast_image_resize`

Lossy color conversion:

- SharpYUV

WebP encode/mux:

- `libwebp`
- mux/container API для ICC/EXIF/XMP

### 13.3 Требование к интеграции

Утилита должна использовать прямую библиотечную интеграцию.

Shell-вызовы `cwebp` допустимы только как временная отладочная опора, но не как
основная production-архитектура.

### 13.4 Библиотечная граница

Пакет предоставляет библиотеку и CLI поверх одного пайплайна:

- `convert(&[u8], &ConversionOptions) -> Result<Vec<u8>>` — основной API без
  зависимости от файловой системы;
- `convert_file(input, output, &ConversionOptions)` — файловый адаптер;
- CLI отвечает только за разбор аргументов и вызывает `convert_file`;
- конфигурационные типы принадлежат библиотечному слою и не зависят от `clap`;
- feature `cli` включает `clap` и бинарный target, а library-only сборка доступна
  через `--no-default-features`.

---

## 14. Ошибки и ограничения

Утилита должна завершаться понятной ошибкой при:

- неподдерживаемом формате;
- неподдерживаемой цветовой модели;
- неподдерживаемом ICC/profile class;
- конфликтующих CLI-флагах;
- попытке получить размеры больше поддерживаемого WebP canvas;
- animated input в `v1`.

### 14.1 Ограничение WebP по размеру

Итоговый кадр WebP не должен превышать:

- `16383 x 16383`

Если resize или исходный файл приводят к превышению лимита, это ошибка.

---

## 15. Обязательные инварианты

Утилита считается корректной, если:

- orientation применяется к пикселям до resize и encode;
- resize выполняется в linearized source RGB, а не напрямую в gamma-coded
  пространстве;
- RGBA resize выполняется alpha-aware через
  `premultiply -> resize -> unpremultiply`;
- вход не сводится к `RGBA8` в самом начале пайплайна;
- lossy path по умолчанию использует high-bit-depth `simpleyuv`;
- lossless и near-lossless path используют `ARGB8` encode path;
- финальная квантовка до 8 бит происходит как можно позже;
- ICC сохраняется, если это разрешено metadata policy и профиль всё ещё
  соответствует финальным пикселям;
- hidden RGB под transparent pixels не считается стабильным контрактом после
  resize;
- неподдерживаемые color spaces не обрабатываются "как попало", а явно
  отклоняются.

---

## 16. Минимальный план реализации

### Этап 1

- CLI skeleton
- decode JPEG/PNG/WebP
- metadata extraction
- EXIF orientation normalize
- RGB ICC handling через `lcms2`
- linear-light resize через `fast_image_resize`
- premultiply/unpremultiply alpha
- lossless WebP encode
- ICC attach

### Этап 2

- lossy WebP path через `libwebp`
- SharpYUV integration
- `-q`, `-m`, `-sns`, `-f`, `-alpha_q`, `--sharpyuv` / `--sharp_yuv`

### Этап 3

- near-lossless
- `-metadata` policy
- расширение поддержки форматов

---

## 17. Минимальный набор acceptance tests

Обязательные тестовые сценарии:

- JPEG с embedded ICC roundtrip без resize;
- JPEG/PNG с non-sRGB RGB profile и resize без потери соответствия профилю;
- EXIF orientation корректно нормализуется и не приводит к double-rotation;
- PNG с полупрозрачными краями не даёт цветных ореолов после resize;
- lossy path по умолчанию действительно использует high-bit-depth `simpleyuv`;
- `--sharpyuv` действительно переключает lossy path на SharpYUV;
- `16-bit` вход не теряет точность раньше финальной квантовки;
- `-lossless` и `-near_lossless` конфликтуют корректной ошибкой;
- `--width/--height`, `--max-side` и `--max-width/--max-height` конфликтуют
  корректной ошибкой;
- WebP output over limit `16383 x 16383` отклоняется корректной ошибкой.

---

## 18. Итоговый результат

На выходе `img2webp-hq` должен давать:

- корректно декодированное изображение;
- корректно ориентированное изображение;
- качественно отресайзенное изображение при необходимости;
- корректно обработанную alpha;
- сохранённый ICC-профиль, если это разрешено политикой metadata;
- WebP выбранного режима кодирования;
- максимально качественный результат в рамках реальных ограничений WebP.
