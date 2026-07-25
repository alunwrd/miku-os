# NVIDIA GB206 (Blackwell): RTX 5060 Ti / 5060, драйвер и план загрузки GSP

Документ описывает драйвер GeForce RTX 5060 Ti / 5060 (silicon GB206) в
`kernel/drivers/gpu/nvidia/rtx5060/` и пошаговый план доведения до полного запуска GSP-RM.
Парный документ для Turing: `docs/Nvidia_tu116.md`.

ABI сверен с:
- nouveau (linux master, ветка r570): `nvkm/subdev/fsp/{gh100.c, gb202.c, base.c}`,
  `nvkm/subdev/gsp/{gh100.c, gb202.c}`, `include/nvhw/ref/gh100/dev_fsp_pri.h`
- open-gpu-kernel-modules 570.144
- nova-core (патчи "FSP Chain of Trust boot")

---

## 1. Чем Blackwell отличается от Turing

На Turing хост сам гоняет код на falcon-ах: FWSEC-FRTS лочит WPR2, booter на
SEC2 DMA-ит образ GSP-RM и стартует RISC-V. На Hopper/Blackwell всей цепочкой
управляет **FSP** (Firmware Security Processor) - отдельный always-on
процессор, который загружается сам на devinit, до того как ОС вообще увидела
карту.

Роль хоста сжимается до трёх шагов:

```
1) застейджить в sysmem: GSP-FMC (ELF из linux-firmware), GSP-RM (radix3),
   GSP_FMC_BOOT_PARAMS
2) отправить FSP одно сообщение NVDM_TYPE_COT (chain of trust) по
   MCTP-over-EMEM: адреса образов + hash/pubkey/signature FMC
3) дождаться secure-boot-complete; FSP верифицирует FMC, FMC карвит FRTS,
   ставит WPR2, грузит GSP-RM и отпускает RISC-V lockdown
```

SEC2 в загрузке не участвует вообще. После старта GSP-RM всё как на Turing:
CMDQ/MSGQ, RPC `SET_SYSTEM_INFO` -> `SET_REGISTRY` -> `GET_GSP_STATIC_INFO`,
поэтому слои `gtx1650/msgq.rs` и `gtx1650/rpc.rs` переиспользуемы.

Ключевые константы GB20x (nouveau `fsp/gb202.c`):
- COT **version 2** (Hopper был v1)
- hash 48 B, public key 97 B, signature 96 B (ECDSA P-384 вместо RSA3K)

---

## 2. Карта модулей

| Файл | Что делает |
|------|------------|
| `rtx5060/mod.rs` | device-id match (0x2Dxx), имена SKU, вход init |
| `rtx5060/regs.rs` | PFSP EMEM/очереди, THERM scratch, GSP falcon, MCTP/NVDM константы |
| `rtx5060/fsp.rs` | транспорт: EMEM PIO, send_sync (команда + ответ), wait_secure_boot |
| `rtx5060/fmc.rs` | `NVDM_PAYLOAD_COT` (packed, 860 B, v2), парсер FMC ELF32, self-test |
| `rtx5060/boot.rs` | фазы 1-3: стейджинг FMC/GSP-RM/bootloader, WPR-meta r570, `GSP_FMC_BOOT_PARAMS`, отправка COT, поллинг lockdown + `GSP_INIT_DONE` |
| `rtx5060/device.rs` | зарегистрированный девайс + статус прошивок |
| `rtx5060/init.rs` | оркестрация bring-up (6 стадий, все non-fatal) |
| `nvidia/gsp_common/rpc.rs` | chip-independent RPC-транспорт r535/r570: фрейминг элементов очереди (48 B заголовок, XOR-checksum, elem_count), doorbell (falcon+0xC00), continuation records |
| `nvidia/gsp_common/sysinfo.rs` | `GspSystemInfo` r570 (928 B, зеркало C-лейаута), PACKED_REGISTRY_TABLE, секвенсер first_contact |

Переиспользуется из общего слоя: `mmio.rs`, `pci.rs`, `msi.rs`, `chip.rs`
(GB2xx codename-ы добавлены), `fwload.rs` (график /lib/firmware с диска).

Переиспользовано из `gtx1650/`: `dma_buf.rs` (phys-contig сysmem),
`gsprm.rs` (radix3, `GspFwWprMeta` - r570-блок те же 256 байт, парсеры
gsp-<line>.bin и bootloader-<line>.bin), `bootargs.rs` (libos-таблица,
rmargs, CMDQ/MSGQ).

Сверено с сырыми хедерами nouveau (не с саммари):
- `THERM_I2CS_SCRATCH` = **0xAD00BC** на GB202+ (`nvhw/ref/gb202/dev_therm.h`).
  На GH100 было 0x200BC; чтение старого смещения даёт 0x00 на живой POST-нутой
  карте - так и был пойман первоначальный баг "scratch 0x00".
- FMC - это **ELF32** (nouveau `gsp/gh100.c` сверяет заголовок с эталонным
  `elf32_hdr`), а не ELF64. Секции: image / hash / signature / publickey.
- `frtsVidmemOffset` в COT - смещение **от конца FB**, не абсолютный адрес:
  `ALIGN(rsvd, 0x200000)`, где rsvd = ALIGN(heap_non_wpr 0x220000 +
  pmuReserve 0x1820000, 2 MiB) = 0x1C00000 (не требует знания размера VRAM).
- бит lockdown в `FALCON_HWCFG2` (bit 13) - как на ga102+, ещё VERIFY.

---

## 3. Прошивки

Прошивки NVIDIA живут в `kernel/drivers/gpu/nvidia/firmware/`, по папке на карточку
(см. `kernel/drivers/gpu/nvidia/firmware/README.md`). Builder стейджит их как
`/lib/firmware/...` и в `boot/firmware.img` внутри ISO, и на корневой
`disk.img`:

```
kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp/fmc-570.144.bin         ->  /lib/firmware/nvidia/gb206/gsp/fmc-570.144.bin
kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp/bootloader-570.144.bin  ->  /lib/firmware/nvidia/gb206/gsp/bootloader-570.144.bin
kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp/gsp-570.144.bin         ->  /lib/firmware/nvidia/gb206/gsp/gsp-570.144.bin
```

Три файла - тот же набор, что `NVKM_GSP_FIRMWARE_FMC(gb206, 570.144)` в
nouveau. Папка rtx5060ti гитигнорится (GSP-RM весит ~61 MB). Источник:
linux-firmware >= 20250509 (ветка 570.144), каталог `nvidia/gb206/gsp/`, либо
системный `/usr/lib/firmware/nvidia/gb206/gsp/`. FMC - это ELF32 с секциями
`image`, `hash`, `signature`, `publickey`; init.rs валидирует размеры
крипто-материала (48/97/96) прямо на загрузке. gsp-570.144.bin общий для
многих чипов; для GB20x из него берётся секция `.fwsignature_gb20x`.
bootloader-570.144.bin - NVFW-контейнер с `RM_RISCV_UCODE_DESC` v5
(monitorData=0xa00, monitorCode=0xb200 в текущей ветке).

---

## 4. Что уже сделано

- Детект GB206 по PCI (0x2D04 = 5060 Ti, 0x2D05 = 5060, ноутбучные 0x2D18/19)
  и по PMC_BOOT_0 (arch 0x1B impl 0x6); диспетчер в `nvidia/mod.rs` уводит
  Blackwell с generic-пути на свой init.
- Host bring-up: BAR0, bus mastering, MSI/MSI-X walk, PTIMER liveness.
- FSP транспорт целиком: EMEM PIO (EMEMC/EMEMD c автоинкрементом), сборка
  MCTP+NVDM пакета, send_sync с ожиданием потребления команды и разбором
  NVDM_TYPE_FSP_RESPONSE (включая NACK-коды), ожидание secure boot.
- `NVDM_PAYLOAD_COT` v2 бит-в-бит с nouveau (compile-time assert 860 байт),
  парсер FMC ELF, проверка крипто-размеров GB20x.
- Стейджинг-проба прошивок с диска, диагностика в serial-лог.
- **Фазы 1-3 (boot.rs)**: стейджинг FMC image + GSP-RM (.fwimage, radix3,
  подпись gb20x) + bootloader; libos/CMDQ-MSGQ boot-args (общие с Turing);
  `GspFwWprMeta` в r570-стиле (host заполняет только sysmem-указатели и
  размеры, FB-смещения ставит сам FMC - offset_set_by_acr); packed
  `GSP_FMC_BOOT_PARAMS` (80 B, compile-time assert); отправка
  `NVDM_TYPE_COT`; поллинг снятия RISCV_BR_PRIV_LOCKDOWN с разбором
  MAILBOX0/1 (0xbadf41xx = busy, адрес boot-params = ok, иначе код ошибки
  FMC); ожидание `GSP_INIT_DONE` в MSGQ. Всё staged-состояние пинится
  до перезагрузки.
- **Фаза 4 (первая половина, gsp_common/)**: RPC-транспорт по CMDQ/MSGQ
  (nouveau rm/r535/rpc.c бит-в-бит: 48-байтовый заголовок элемента,
  XOR-fold checksum по выровненному элементу, поэлементные страничные
  указатели с кросс-подключением, doorbell записью 0 в falcon+0xC00,
  сборка CONTINUATION_RECORD); секвенсер `first_contact`:
  дренаж boot-событий -> `GSP_SET_SYSTEM_INFO` (72) -> `SET_REGISTRY` (73).
  Обе команды fire-and-forget (nouveau шлёт их с REPLY_NOSEQ, ответа GSP
  не постит) - ошибки всплывают асинхронно событием OS_ERROR_LOG.
- Шелл: `nvidia gb206`, `nvidia fsp`, `nvidia cot-dryrun`,
  `nvidia gb206-boot`, `nvidia gb206-rpc`.

Что не сделано (фаза 4b): `GET_GSP_STATIC_INFO` (65) - это REPLY_RECV
вызов, нужно зеркало GspStaticConfigInfo r570 (большая структура: gid,
SKUInfo, fbRegionInfo, engine caps), парс имени GPU и размера FB из
ответа. Транспорт для него уже готов (`GspRpc::call`). Также перенос
bootargs.rs/gsprm.rs из gtx1650/ в gsp_common/ отдельным коммитом.

---

## 5. План разработки

### Сделано (фазы 0-4a)

| Фаза | Что | Где |
|------|-----|-----|
| 0 | hardware-loop: PMC_BOOT_0, PTIMER, FSP scratch (0xAD00BC), очереди | `init.rs` |
| 1 | sysmem-стейджинг: FMC image, .fwimage + radix3, bootloader, libos/CMDQ-MSGQ, WPR-meta r570, GSP_FMC_BOOT_PARAMS | `boot.rs` |
| 2 | отправка NVDM_TYPE_COT, разбор ответа FSP, поллинг lockdown + MAILBOX0/1 | `boot.rs` |
| 3 | handshake: ожидание GSP_INIT_DONE в MSGQ | `boot.rs` |
| 4a | RPC-транспорт (фрейминг, checksum, doorbell, continuation) + SET_SYSTEM_INFO + SET_REGISTRY | `gsp_common/` |

Дальше - полный оставшийся план. Зависимости линейные (4b -> 5 -> 6 -> 7),
после фазы 7 ветки 8/9/10 независимы друг от друга.

### Фаза 4b: GET_GSP_STATIC_INFO - закрыть критерий "GSP загружен"

Первый REPLY_RECV вызов (функция 65). Транспорт готов (`GspRpc::call`),
не хватает только структуры ответа.

- Зазеркалить `GspStaticConfigInfo` из r570 `nvrm/gsp.h` (как сделано для
  `GspSystemInfo`): grCapsBits[23], gidInfo (0x100+8), SKUInfo,
  fbRegionInfoParams (16 регионов по 88 B), sriovCaps, engineCaps,
  poisonFuseEnabled, ecidInfo[2], fwWprLayoutOffset. Compile-time assert
  на размер; запрос - зануленная структура того же размера, ответ -
  заполненная.
- Распарсить и закешировать в `Gb206BootState`: имя GPU (SKUInfo /
  gidInfo), реальный fbSize по регионам, nonWprHeapOffset/frtsOffset
  (fwWprLayoutOffset - сверка с тем, что мы посчитали хостом), маску
  движков.
- Расширить `nvidia gb206-rpc` (или новая `nvidia gb206-info`): печать
  имени, VRAM, движков. Это пункт 4 критерия из раздела 7 - после него
  загрузка GSP формально полная.
- Сюда же: декодер кодов NV_STATUS для NACK-ов (0x51 NO_MEMORY, 0x55
  NOT_READY, 0x66 TIMEOUT_RETRY, ...) вместо сырых hex в логе.

### Фаза 5: рефакторинг gsp_common (без нового функционала)

Механический перенос chip-independent кода из `gtx1650/` в
`nvidia/gsp_common/`, отдельным коммитом, оба драйвера переводятся на
общие пути:

- `dma_buf.rs` -> `gsp_common/dma_buf.rs` (нужен всем).
- `bootargs.rs` -> `gsp_common/bootargs.rs` (libos-таблица, rmargs,
  кольца - идентичны r535/r570).
- Из `gsprm.rs` выделить chip-independent часть: radix3, `GspFwWprMeta`,
  парсеры gsp-<line>.bin / bootloader-<line>.bin (`parse_gsp_rm_sig`,
  `parse_gsp_bootloader`, `NvfwBinHdr`). Turing-специфика (booter, WPR
  layout сверху вниз, SEC2) остаётся в `gtx1650/`.
- Починить константы в `gtx1650/rpc.rs` (номера функций там от ранних
  экспериментов: 1/2/3 вместо 72/65/73) либо удалить его в пользу
  `gsp_common/rpc.rs` целиком.
- Критерий: `cargo build` чистый, `nvidia gsp-rm-*` (Turing) и
  `nvidia gb206-*` работают как раньше.

### Фаза 6: RM-объекты - клиент, девайс, GSP_RM_CONTROL

Всё общение после static info идёт через три RPC: `GSP_RM_ALLOC` (103),
`GSP_RM_CONTROL` (76), `FREE` (10). Референс: nouveau
`rm/r535/{alloc.c, ctrl.c, client.c, device.c}`.

- Зазеркалить заголовки пейлоадов: `rpc_gsp_rm_alloc_v03_00`
  (hClient/hParent/hObject/hClass/status + params) и
  `rpc_gsp_rm_control_v03_00` (hClient/hObject/cmd/status/paramsSize +
  params).
- Дерево хэндлов как в nouveau: hClient = 0xc1d00000, затем
  NV01_ROOT (клиент) -> NV01_DEVICE_0 (0x0080) -> NV20_SUBDEVICE_0
  (0x2080). Простому аллокатору хэндлов хватит счётчика.
- Первые control-вызовы поверх subdevice:
  `NV2080_CTRL_CMD_GPU_GET_NAME_STRING` (0x20800110) - имя из самого RM,
  `NV2080_CTRL_CMD_THERMAL_...` / `NV2080_CTRL_CMD_PERF_...` - температура
  и клоки для `nvidia temp` на Blackwell.
- Шелл: `nvidia gb206-client` (alloc root/device/subdevice + пара
  контролов, печать результатов).
- Критерий: GET_NAME_STRING возвращает "NVIDIA GeForce RTX 5060 Ti".

### Фаза 7: прерывания вместо поллинга

Сейчас всё на PTIMER-поллинге; для событий и каналов нужен MSI-X.

- Включить MSI-X (каркас в `nvidia/msi.rs` уже есть: capability walk),
  вектор 0 -> обработчик в IDT MikuOS.
- Blackwell-часть: дерево прерываний GSP-эры - CPU_INTR_TOP /
  CPU_INTR_LEAF (nouveau `nvkm/subdev/mc/ga100.c` и `tu102.c` intr paths);
  для GSP-RM хватает leaf-бита msgq notify.
- Обработчик: по прерыванию дренировать MSGQ (`drain_events`),
  диспатчить в таблицу подписок (аналог `r535_gsp_msg_ntfy_add`):
  OS_ERROR_LOG -> serial, RC_TRIGGERED -> лог + пометка канала,
  MMU_FAULT_QUEUED -> лог.
- Зарегистрировать событие через `NV01_EVENT_KERNEL_CALLBACK_EX` на
  subdevice (nouveau r535_gsp_intr_get_table + event alloc).
- Критерий: `nvidia gb206-rpc` работает без единого spin-поллинга MSGQ.

### Фаза 8: память - BAR1/BAR2, GMMU, FB-хип

Нужно для любых каналов и DMA с GPU-стороны.

- Распарсить fbRegionInfo из static info, поднять простой аллокатор
  VRAM (bump/buddy поверх незанятого региона под WPR).
- VMM: зазеркалить new-style GMMU v3 PDE/PTE (Hopper+ формат тот же,
  что Ada, nouveau `nvkm/subdev/mmu/vmmgp100.c` + `vmmtu102.c` семейство;
  для Blackwell сверить радиус PDE по gb202 headers).
- BAR2 (inst) страницы для channel instance blocks; BAR1 окно для
  отображения VRAM в CPU (пока не критично - фреймбуфер можно держать
  в sysmem через GMMU).
- RPC-часть: `SET_PAGE_DIRECTORY` (54) / `UPDATE_PDE_2` (53) для
  привязки адресного пространства канала.
- Критерий: выделить VRAM-буфер, замапить его в GMMU, прочитать через
  BAR1 то, что записали через GMMU-адрес (или CE-копией в фазе 9).

### Фаза 9: FIFO-канал и первая реальная работа GPU (CE-копия)

- Alloc-цепочка через GSP_RM_ALLOC: `FERMI_VASPACE_A` (0x90f1),
  channel group (`KEPLER_CHANNEL_GROUP_A` 0xa06c), GPFIFO-канал
  Blackwell-класса (`BLACKWELL_CHANNEL_GPFIFO_A` 0xc96f, id сверить по
  ogkm 570.144), CE-объект (`BLACKWELL_DMA_COPY_A` 0xc9b5, сверить).
  Референс: nouveau `rm/r535/fifo.c` + `rm/gb20x.c` (engine list).
- USERD + GPFIFO ring в sysmem, doorbell работы через NV_CHRAM /
  runlist doorbell (у GSP-RM это `NV2080_CTRL_CMD_FIFO_...` +
  work submit token из ответа alloc-а).
- Пуш-буфер: методы DMA-copy (OFFSET_IN/OUT, LINE_LENGTH, LAUNCH_DMA) -
  скопировать 4 KiB sysmem -> VRAM -> sysmem, сверить паттерн.
- Шелл: `nvidia gb206-ce-test`.
- Критерий: паттерн совпал - GPU впервые исполнил нашу работу.

### Фаза 10: дисплей (опционально, но это "лицо" MikuOS)

- `rm/r570/disp.c`: alloc `NVC372_DISPLAY_SW`, `NVC670_DISPLAY`-класс
  Blackwell (сверить id), enumerate коннекторов через
  `NV0073_CTRL_CMD_...`, modeset через `SetMode`-цепочку RM.
- Скан-аут из VRAM-буфера фазы 8; вывод консоли MikuOS через GPU
  вместо GOP-фреймбуфера.
- Критерий: картинка по HDMI/DP с 5060 Ti.

### Фаза 11: жизненный цикл и устойчивость

- Teardown: `UNLOADING_GUEST_DRIVER` (47) при reboot/shutdown, чтобы
  карта не оставалась в полубутнутом состоянии для kexec/тёплого ребута.
- Повторный `gb206-boot` без перезагрузки: resume-ветка COT
  (frts 0/0, sr-meta) - у nouveau это suspend/resume путь.
- Watchdog: таймауты RPC -> RC-recovery лог, счётчики ошибок в
  `/proc`-подобном выводе `nvidia gb206`.
- Логи GSP: маппинг LOGINIT/LOGINTR/LOGRM буферов (уже выделены в
  bootargs) в читаемый вид - `nvidia gb206-log`, декодер libos log
  (nouveau `r535_gsp_libos_debugfs_init` формат).

### Сводка приоритетов

```
4b  static info          - маленькая, закрывает критерий загрузки
5   рефакторинг          - дёшево, пока кода мало
6   RM-объекты           - открывает весь control-plane
7   прерывания           - качество жизни + требование каналов
8   память               - фундамент для 9/10
9   CE-копия             - первое настоящее исполнение на GPU
10  дисплей              - видимый результат
11  lifecycle            - зрелость драйвера
```

---

## 6. Команды шелла

| Команда | Действие |
|---------|----------|
| `nvidia gb206` | статус карты: чип, FSP, наличие прошивок, состояние boot |
| `nvidia fsp` | указатели очередей FSP + boot-скретч (read-only) |
| `nvidia cot-dryrun` | сборка/валидация COT-пейлоада без отправки, парс FMC с диска |
| `nvidia gb206-boot` | полная загрузка GSP-RM: стейджинг, COT, lockdown, INIT_DONE |
| `nvidia gb206-rpc` | first-contact RPC после boot: дренаж событий, SET_SYSTEM_INFO, SET_REGISTRY |
| `nvidia list` | карта видна и в общем списке |

Плановые (появляются по фазам раздела 5):

| Команда | Фаза | Действие |
|---------|------|----------|
| `nvidia gb206-info` | 4b | GET_GSP_STATIC_INFO: имя GPU, VRAM, движки, WPR-раскладка от RM |
| `nvidia gb206-client` | 6 | alloc root/device/subdevice + GET_NAME_STRING, температура |
| `nvidia gb206-ce-test` | 9 | CE-копия sysmem -> VRAM -> sysmem со сверкой паттерна |
| `nvidia gb206-log` | 11 | декодер libos-логов GSP (LOGINIT/LOGINTR/LOGRM) |

---

## 7. Критерии

### "Полная загрузка GSP достигнута" (закрывается фазой 4b)

1. FSP принял NVDM_TYPE_COT (ответ FSP_RESPONSE с err=0, не NACK);
2. RISCV_BR_PRIV_LOCKDOWN снялся, MAILBOX0 == 0;
3. GSP-RM выложил `GSP_INIT_DONE` в MSGQ;
4. `SET_SYSTEM_INFO` принят, `GET_GSP_STATIC_INFO` вернул имя GPU и размер FB.

Пункты 1-3 плюс отправка SET_SYSTEM_INFO/SET_REGISTRY уже реализованы
(`gb206-boot` + `gb206-rpc`); пункт 4 закрывает фаза 4b.

### "Драйвер functional" (дальний ориентир)

5. GSP_RM_ALLOC/CONTROL работают: GET_NAME_STRING отвечает (фаза 6);
6. события приходят по MSI-X без поллинга (фаза 7);
7. CE-канал исполнил копию, паттерн сверен (фазы 8-9);
8. teardown при reboot не оставляет карту в полубутнутом состоянии (фаза 11).
