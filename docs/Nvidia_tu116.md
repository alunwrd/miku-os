# NVIDIA TU116 / TU117 (Turing): драйвер и план полной загрузки GSP

Документ описывает текущее состояние драйвера GeForce GTX 1650 / 1660 (silicon
TU117 и TU116) в `kernel/drivers/gpu/nvidia/gtx1650/`, конвейер загрузки GSP-RM и пошаговый
план доведения до полного запуска GSP с последующим использованием GPU.

Все ссылки на ABI сверены с:
- nouveau: `drivers/gpu/drm/nouveau/nvkm/subdev/gsp/{r535.c, fwsec.c, tu102.c}`
- open-gpu-kernel-modules (ogkm): `kgspBootstrap_TU102`, `kgspExecuteBooterLoad_TU102`
- nova-core 570.144 bindings

---

## 1. Контекст архитектуры

Turing - первое поколение NVIDIA с GSP (GPU System Processor): встроенным
RISC-V ядром, на которое выгружается почти вся настройка GPU. Открытая модель
драйвера (как в nouveau r535 и в ogkm) сводится к тому, что хост:

1. готовит подписанный образ GSP-RM в защищённом регионе VRAM (WPR2),
2. запускает его сигнованным booter-ом на Falcon-движке,
3. дальше общается с работающим GSP-RM по очередям сообщений (CMDQ/MSGQ) через
   RPC.

Без загруженного GSP драйвер ограничен host-side регистрами (температура, PMC,
PTIMER, идентификация). Всё серьёзное (PGRAPH, display, CE, питание) идёт через
GSP-RM.

TU117 и TU116 делят host-side раскладку регистров. Различия:
- поле implementation в `PMC_BOOT_0`: 0x7 (TU117) против 0x8 (TU116)
- диапазон PCI device-id: 0x1Fxx против 0x21xx

Полный конвейер GSP-RM реализован только для TU116 (`tu116::matches`).

---

## 2. Карта модулей

| Слой | Файл | Что делает |
|------|------|------------|
| Оркестрация init | `init.rs` | PCI bind, BAR map, chip ID, MSI, PTOP-walk, VBIOS, staging GSP-RM |
| Falcon-движок | `falcon.rs` | IMEM/DMEM PIO upload, soft reset, halt-poll, HWCFG liveness |
| FBIF | `fbif.rs` | ctxdma / TRANSCFG для DMA Falcon-а из sysmem |
| DMA-буферы | `dma_buf.rs` | phys-contiguous sysmem, write-barrier |
| VBIOS | `vbios.rs` | expansion ROM + PRAMIN-shadow, BIT, образы, токены, VbiosView remap |
| GSP-RM staging | `gsprm.rs` | ELF-парсер gsp-570.144, radix3, WPR-meta (256B), оркестратор `boot` |
| Boot args | `bootargs.rs` | libos table, rmargs, log-регионы, CMDQ/MSGQ shared region + PTE |
| Очереди | `msgq.rs` | кольца CMDQ/MSGQ |
| RPC | `rpc.rs` | фрейминг RpcHeader (32B) + функции NV_VGPU_MSG_* |
| SEC2 ACR | `sec2.rs` | AHESASC v1/v2, booter_load на SEC2 |
| FWSEC | `fwsec.rs` | FRTS-команда, лочит WPR2 |
| NVDEC | `nvdec.rs` | scrubber региона перед локом |
| Прошивки | `tu116_fw.rs` | embedded blob set + NVFW-контейнеры |
| Прочее | `pmc.rs`, `therm.rs`, `regs.rs`, `quirks.rs` | гейты, термосенсор, регистры, per-chip маски |

---

## 3. Конвейер загрузки GSP-RM (как он собран сейчас)

Точка входа: `gsprm::boot(bar0, gpu)`, команда шелла
`nvidia gsp-rm-boot-full`. Шесть стадий, каждая non-fatal (устройство остаётся
зарегистрированным и инспектируемым при любой частичной неудаче):

```
1) NVDEC scrubber        обнуляет регион VRAM, который залочит WPR2
2) gsprm::load           парс ELF -> .fwimage в phys-contig sysmem,
                         radix3 page table, WPR-meta block, bootloader,
                         signature; всё пинится в GSP_RM
3) FWSEC FRTS на GSP      сигнованный ucode из VBIOS карвит FRTS-регион и
                         программирует NV_PFB_PRI_MMU_WPR2_ADDR_LO/HI
4) GATE: WPR2 locked?     читаем PFB; если лок не встал - STOP с диагностикой
5) boot args + booter     строим libos/rmargs/CMDQ-MSGQ, libos-адрес в GSP
                         MAILBOX0/1, booter_load на SEC2 DMA-ит radix3-образ
                         в залоченный WPR2 и стартует RISC-V; затем GSP
                         FALCON_OS <- app_version
6) GSP handshake          поллим GSP-owned MSGQ в shared-регионе на первое
                         сообщение GSP-RM (событие GSP_INIT_DONE)
```

Ключевой архитектурный момент: лок WPR2 делает **FWSEC FRTS** (стадия 3), а не
SEC2 AHESASC. Booter на SEC2 (стадия 5) только DMA-ит образ в уже залоченный
регион и стартует ядро. Это соответствует порядку `kgspBootstrap_TU102`.

### Что уже корректно по ABI

- `GspFwWprMeta`: ровно 256 байт, `#[repr(C)]`, compile-time assert, magic
  `0xdc3aae21371a60b3`, revision 1. Сверено с ogkm `gsp_fw_wpr_meta.h`.
- radix3: трёхуровневая таблица 512 u64/страница, проверка резолва page0
  (`radix3_resolves`).
- boot args: PTE-массив shared-региона, CMDQ tx-header (host-producer),
  rmargs (GSP_ARGUMENTS_CACHED, 80B), libos[4] (LOGINIT/LOGINTR/LOGRM/RMARGS),
  log-регионы с собственными PTE на offset 8. Есть `self_test` без запуска
  Falcon.
- RPC: header_version 0x03000000, signature 'VGPU' (0x47565055).

---

## 4. Реальные блокеры (по убыванию важности)

### Блокер 1: devinit не реализован

Самый фундаментальный. В `init.rs:14` помечен как "Not yet done". Без прогона
init-скрипта из VBIOS init-table (токен `I` уже находится в `init.rs:138`):

- PFB MMU не запрограммирован -> `vram_info()` может вернуть мусор или ноль
  (`GsprmError::NoVram`), и вся layout-математика WPR2 рушится
- PTIMER не откалиброван (в логах "not programmed")
- клоки и термолимиты не выставлены
- SEC2/GSP Falcon могут быть gated

Частичный обход: если карта была POST-нута материнкой как primary display,
devinit уже отработал прошивкой VGA. На secondary/headless пути - нет. Это
определяет, достоверны ли вообще стадии 1-4 оркестратора. Детект: PTIMER
num/denom != 0 означает, что devinit уже прошёл.

### Блокер 2: лок WPR2 через FWSEC FRTS не подтверждён на железе

Весь код `fwsec::run_frts` написан и сверен с nouveau, но это "expected to halt
early", пока не прогнано на реальном TU116. Стадия 4 - главный hard gate.
Проверки: `frts_err` (топ 16 бит scratch `0x1438`) должен быть 0, и
`PFB_PRI_MMU_WPR2_ADDR_LO/HI` должны декодироваться в непустое окно.

### Блокер 3: нет пост-handshake RPC-секвенсера

После `GSP_INIT_DONE` настоящий драйвер обязан прокачать
`SET_SYSTEM_INFO` -> `SET_REGISTRY` -> `GET_GSP_STATIC_INFO` и дальше control-RPC,
иначе GPU не используется. В `rpc.rs` есть фрейминг и `RpcDriver::send/try_recv`,
но нет машины состояний за пределами первого сообщения и нет GSP doorbell для
сабмита.

### Блокер 4: всё на polling, нет прерываний

MSI не маршрутизируется (ждёт `apic::alloc_msi_vector`), GSP doorbell
(notify/swgen) для command submission не подключён. `pmc::mask_all_interrupts`
держит всё замаскированным. Для устойчивого рантайма нужен MSI-вектор и
диспетчер событий MSGQ вместо поллинга.

### Прочее

- На polling завязаны halt-poll-ы Falcon (timeout-bounded по PTIMER - ок).
- Нет аллокации GPU-объектов через RPC (root/device/subdevice).

---

## 5. План разработки

### Фаза 0: инструментарий и hardware-loop (предпосылка ко всему)

- Прогнать `nvidia gsp-rm-boot-full` на реальном TU116, собрать серийный лог по
  стадиям.
- Дамп ключевых регистров до/после каждой стадии: `PFB_*`,
  `WPR2_ADDR_LO/HI`, FRTS scratch `0x1438`, Falcon mailbox-ы, `FALCON_CPUCTL`.
- Цель: точно знать, на какой стадии и с каким кодом реально останавливается
  boot.

Без этой фазы отладка FWSEC и booter идёт вслепую.

### Фаза 1: devinit (снимает блокер 1)

- Новый модуль `devinit.rs`: интерпретатор init-скрипта Turing из VBIOS
  init-table.
- Opcode-набор по nouveau `nvbios_init`: INIT_IO, INIT_COPY, INIT_REG_*,
  INIT_PLL, INIT_CONDITION, INIT_ZM_REG, INIT_RESUME и др.
- Запускать до `vram_info()`, если PFB/PTIMER читаются непрограммированными;
  пропускать, если карта уже POST-нута (детект по PTIMER num/denom != 0).
- Критерий готовности: `PTIMER scale` показывает реальные MHz, `vram_info()`
  отдаёт корректный размер VRAM.

### Фаза 2: доведение лока WPR2 (блокер 2)

- На основе лога фазы 0 отладить `fwsec::run_frts`: порядок reset GSP Falcon,
  scrub-wait, FBIF slot 4 (sysmem physical), bld-дескриптор v2.
- Подтвердить `frts_err == 0` и `WPR2 LOCKED` в PFB.
- Запасной путь: если FRTS не лочит - портировать полный SEC2 AHESASC
  (`RM_FLCN_ACR_DESC` в DMEM); заготовки уже в `sec2.rs`
  (`attempt_acr_v2`, `build_for_ahesasc`).
- Критерий: стадия 4 оркестратора проходит gate.

### Фаза 3: booter_load -> старт GSP-RM RISC-V (продолжение блокера 2)

- Проверить `sec2::attempt_booter_load`: booter возвращает `mb0 == 0` и реально
  DMA-ит radix3-образ в залоченный WPR2.
- Убедиться, что libos-адрес уходит в GSP MAILBOX0/1 до запуска booter, а
  `FALCON_OS` получает app_version после.
- Критерий: стадия 6 ловит первое сообщение в MSGQ с signature 'VGPU' и
  function `GSP_INIT_DONE`.

### Фаза 4: пост-boot RPC-секвенсер (блокер 3)

- В `rpc.rs` добавить машину состояний:
  `SET_SYSTEM_INFO` (PCI ids, IOMMU, версия драйвера) -> `SET_REGISTRY` ->
  `GET_GSP_STATIC_INFO`.
- Full duplex: запись в CMDQ + bump write_ptr + GSP doorbell, ожидание ответа
  в MSGQ по sequence.
- Распарсить static info (имя GPU, размер FB, bitmap движков) и вывести. Это
  первое реальное доказательство, что GSP-RM живёт.

### Фаза 5: прерывания и устойчивый рантайм (блокер 4)

- Подключить `apic::alloc_msi_vector`, запрограммировать MSI, размаскировать
  GSP-вектор.
- Обработчик GSP doorbell -> диспетчер MSGQ-событий вместо polling.
- Поэтапно демаскировать движковые IRQ в `pmc`.

### Фаза 6: использование GPU через GSP-RM

- Аллокация GPU-объектов через RPC (`NV01_ROOT`, device, subdevice).
- Дальше по потребности проекта: CE для копий, display, либо compute-контекст.

---

## 6. Зависимости фаз

```
 Фаза 0 (лог) ──┬─> Фаза 1 (devinit) ──> Фаза 2 (WPR2 lock)  ──> Фаза 3 (booter)
                │                                                     │
                └──────────────────────────────────────> Фаза 4 (RPC) ┘
                                                              │
                                                    Фаза 5 (IRQ) ──> Фаза 6 (GPU use)
```

Самый дешёвый быстрый выигрыш: **фаза 0 + фаза 1**. Пока devinit не отработал,
стадии 1-4 на secondary-карте недостоверны, и отладка FWSEC идёт вслепую.

---

## 7. Команды шелла для отладки

| Команда | Действие |
|---------|----------|
| `nvidia info` / `status` | базовая информация об устройстве |
| `nvidia falcon` | liveness SEC2/GSP/NVDEC/FECS/GPCCS |
| `nvidia firmware` | парс embedded blob set |
| `nvidia gsp-rm` | состояние staged GSP-RM |
| `nvidia gsp-rm-dryrun` | self-test radix3 + WPR-meta без прошивки |
| `nvidia gsp-rm-load` | стейджинг образа в sysmem |
| `nvidia gsp-bootargs` | self-test boot args (без запуска Falcon) |
| `nvidia nvdec-scrub` | прогон scrubber |
| `nvidia fwsec` (через boot) | FWSEC FRTS |
| `nvidia wpr-state` | чтение WPR2 lock-окна |
| `nvidia gsp-rm-boot-full` | весь конвейер 6 стадий |
| `nvidia next` / `roadmap` | чеклист bring-up с авто-детектом |

---

## 8. Критерий "полная загрузка GSP достигнута"

GSP считается полностью загруженным, когда:

1. WPR2 залочен FWSEC FRTS (`frts_err == 0`, окно PFB непустое);
2. booter_load вернул `mb0 == 0` и заDMA-ил образ в WPR2;
3. GSP-RM выложил `GSP_INIT_DONE` в MSGQ с корректной signature 'VGPU';
4. хост успешно отправил `SET_SYSTEM_INFO` и получил `RPC_OK`;
5. `GET_GSP_STATIC_INFO` вернул осмысленные данные GPU (имя, размер FB,
   bitmap движков).

Пункты 1-3 закрываются фазами 1-3, пункты 4-5 - фазой 4.
