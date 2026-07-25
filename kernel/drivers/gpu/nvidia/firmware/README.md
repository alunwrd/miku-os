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

## Наполнение rtx5060ti/

Нужны три файла из linux-firmware >= 20250509 (ветка 570.144) - тот же
набор, что макрос NVKM_GSP_FIRMWARE_FMC в nouveau:

```
rtx5060ti/gsp/fmc-570.144.bin          # GSP-FMC (chain of trust, ELF32, ~200 KB)
rtx5060ti/gsp/bootloader-570.144.bin   # RM RISC-V monitor (~200 KB)
rtx5060ti/gsp/gsp-570.144.bin          # GSP-RM (~61 MB)
```

Например из git: `https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git`,
каталог `nvidia/gb206/gsp/`.

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
