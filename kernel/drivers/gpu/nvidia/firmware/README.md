# kernel/drivers/gpu/nvidia/firmware/ - прошивки NVIDIA, по папке на карточку

> **Блобы не хранятся в git.** Это проприетарная подписанная микропрограмма
> NVIDIA (GSP-RM, FWSEC, ACR, SEC2, GR ucode) - распространять её решает
> вендор, а не мы. В репозитории лежит только этот README. Сборка без
> блобов работает: образ прошивки просто не создаётся, а GPU сообщает
> "firmware unavailable".

Builder стейджит каждую папку отсюда в `/lib/firmware/<путь чипа>` на
образы (ISO-модуль `boot/firmware.img` и корневой `disk.img`). Раскладка
внутри `/lib/firmware` совпадает с linux-firmware, потому что именно эти
пути запрашивает fwload в ядре. Таблица соответствия папка -> чип живёт
в `builder/src/main.rs` (`NVIDIA_FW_CARDS`); папка без записи в таблице
идет как `/lib/firmware/nvidia/<имя папки>`

| Папка | Карта | Путь на образе | В git |
|-------|-------|----------------|-------|
| `gtx1650/` | GTX 1650 / 1660 (TU116/TU117) | `/lib/firmware/nvidia/tu116/` | нет (gitignore) |
| `rtx5060ti/` | RTX 5060 Ti / 5060 (GB206) | `/lib/firmware/nvidia/gb206/` | нет (gitignore) |

> Имена каталогов здесь — SKU (`gtx1650/`, `rtx5060ti/`), а модули драйвера
> называются по архитектуре (`turing/`, `blackwell/`). Расхождение намеренное:
> эти имена зашиты в `NVIDIA_FW_CARDS` (`builder/src/main.rs`), а сами блобы
> лежат вне git, так что переименование каталогов ничего не улучшает и ломает
> раскладку файлов на диске.

## Наполнение rtx5060ti/

Драйвер говорит с одной версией GSP-RM - 615.78.08 (`dev/gsp-615.md`), и
файлы должны быть ровно этой версии. В linux-firmware для GB20x пока только
570.144, поэтому все четыре файла берутся из пакета драйвера той же версии
(на Arch - `/usr/lib/firmware/nvidia/615.78.08/` из nvidia-open/nvidia-utils)
и из исходников open-gpu-kernel-modules 615.78.08:

```
rtx5060ti/gsp/fmc-615.78.08.bin          # GSP-FMC (chain of trust, ELF32, ~280 KB)
rtx5060ti/gsp/bootloader-615.78.08.bin   # RM RISC-V monitor (~285 KB)
rtx5060ti/gsp/gsp-615.78.08.bin          # GSP-RM, gsp_ga10x.bin пакета (~27 MB)
rtx5060ti/gsp/ucodes-615.78.08.bin       # контейнер NVUCODES, ucodes_ga10x.bin пакета (~58 MB)
```

Собирает их `dev/nvidia/fw_extract.py`: FMC и bootloader он строит из
bindata дерева OpenRM (в linux-firmware-раскладке, которую читают
`FmcImage::parse` и `parse_gsp_bootloader`), GSP-RM и ucodes копирует из
пакета:

```
python3 -I dev/nvidia/fw_extract.py --tree refsrc/openrm \
    --driver /usr/lib/firmware/nvidia/615.78.08 \
    --out kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp
```

Файлы 570.144 из папки убраны (builder кладёт на образ всё, что в ней
лежит) и хранятся в `refsrc/firmware-570.144/`. Образ, собранный с этими
файлами, - для своей машины: перед публикацией нужно прочитать лицензию
пакета.

## Другие GB20x (GB202/203/205/207)

Чип опознаётся по `PMC_BOOT_42` (`blackwell/chips.rs`), и каждый ищет свой
каталог, как в linux-firmware: `nvidia/gb202`, `nvidia/gb203`, `nvidia/gb205`,
`nvidia/gb207`. Файлы у всего семейства одни и те же: `gsp_ga10x.bin` - общий
образ, подпись GB20x в секции `.fwsignature_gb20x`. Для такой карты нужна
своя папка с теми же четырьмя файлами и строка в `NVIDIA_FW_CARDS`, например
`("rtx5090", "nvidia/gb202")`.

## Добавление новой карточки

1. Создать папку, например `rtx4070/`, положить блобы с раскладкой как в
   linux-firmware внутри каталога чипа (`gsp/...`, `acr/...`).
2. Добавить строку в `NVIDIA_FW_CARDS` в `builder/src/main.rs`,
   например `("rtx3070", "nvidia/ga104")`.
3. Большие блобы добавить в `.gitignore`.

После этого `cd builder && cargo run` кладёт файлы:
- в `boot/firmware.img` внутри ISO (GRUB-модуль, работает и на реальном
  железе, это основной путь);
- на корневой `disk.img`, если он открывается host-овым debugfs (образ,
  отформатированный самим MikuOS, e2fsprogs не читает; тогда прошивки
  берутся из ISO-модуля, это нормально).

Проверка внутри MikuOS: `nvidia gb206` покажет блобы как present,
`nvidia cot-dryrun` распарсит FMC и проверит крипто-материал.
