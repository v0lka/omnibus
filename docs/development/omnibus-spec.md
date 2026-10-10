# Omnibus — агентный контракт ОС (спека прототипа v1.0)

> Итог вайб-ресёрча «AI-native Linux» (2 итерации): общая картина отрасли + спецификация прототипа. Все решения подтверждены веб-ресёрчем от 09.10.2026; пункты из памяти помечены. Архитектурная формализация (SPI, матрица платформ, политика capabilities и деградации) — в [omnibus-architecture.md](omnibus-architecture.md) v1.0; этот документ остаётся продуктовым слоем и согласован с ним: плоская картина «провайдеров» ранних итераций формализована там в ядро + сменные OS-адаптеры.

## Принятые решения

- **Кому**: разработчики / power users (десктоп-профили целевых ОС; серверные/headless — вне v1)
- **Фокус**: агентный контракт ОС — единый семантический API поверх сменных OS-адаптеров за границей SPI (`omnibus-contracts`), экспортируемый как MCP
- **Инференс**: ноль модельных вызовов в omnibusd — инференс целиком в контуре агента (локальная или облачная модель — выбор юзера на стороне его агента)
- **Формат**: спека прототипа
- **Компоситоры**: X11 + Wayland поровну (Linux-цели); macOS — собственный оконный стек без компоситорных допущений
- **Стратегия**: сразу нативные адаптеры за SPI (без фазы обёрток); Arch — beachhead с полным контрактом (T1), остальные платформы — по матрице ниже
- **Язык**: Rust
- **Платформы v1 (десктоп)**: Arch **T1** · Manjaro **T1** (своя тест-матрица) · Debian trixie **T2** / bookworm **T2** (на полу) / bullseye **T3** · Ubuntu LTS **T2** · Mint **T2** · Fedora **T2** · macOS **T2** (вторая цель после победы на Arch)
- **Ограничения**: ядра ОС не трогаем; максимум переиспользования имеющихся компонентов; `capabilities()` — единственный источник истины о фичах конкретной машины

## 1. White space (почему это стоит строить)

За 2025–2026 появилось ~8 фрагментированных MCP-серверов для Linux: GUI ([linux-desktop-mcp](https://github.com/lotnikov/linux-desktop-mcp) — AT-SPI2, с разбором ловушек XKB/клипборда; [computer-use-linux](https://hysenlabs.com/en/projects/agent-sh-computer-use-linux) — Rust, Wayland-порталы+ydotool; [desk-mcp](https://github.com/ShibbityShwab/desk-mcp); [agent-desktop/xa11y](https://agent-desktop.dev/) — кроссплатформ; [kwin-mcp](https://pypi.org/project/kwin-mcp/) — headless CI) и системные ([RHEL Lightspeed linux-mcp-server](https://rhel-lightspeed.github.io/linux-mcp-server/tools/logs/) — дистровендор уже вошёл; [journald-mcp](https://www.mcpworld.com/en/detail/a47621a7151d97fe1ef6c122a18996c7); [system-monitoring-mcp](https://github.com/friendlygeorge/system-monitoring-mcp-server)). На macOS белое пятно уже частично занято ([Peekaboo](https://peekaboo.sh/) — AX-инспекция + скриншоты + MCP; россыпь AppleScript-MCP) — но и там никто не даёт единого контракта со скоупами и аудитом. Ни один (ни на Linux, ни на macOS) не даёт: единый контракт «ОС как провайдер», скоупы/аудит, пакеты в дистре. Агентная стандартизация (MCP, ACP, AGENTS.md, llms.txt — всё под Linux Foundation/AAIF) живёт над ОС; системный слой свободен.

## 2. Архитектура (всё userspace)

```
MCP-хосты: Claude Code / goose / Cursor (+ любые MCP-клиенты)
     │  «большой мозг»: облачные модели по желанию юзера
omnibusd (omnibus-core) — единый MCP-фасад (stdio; инстанс = child
     │  MCP-клиента), сессия = соединение, скоупы,
     │  аудит-пайплайн, реестр capabilities(), doctor
     │  граница SPI: Rust-трейты + DTO + константы capabilities в крейте
     │  omnibus-contracts — единственная общая зависимость, ноль OS-зависимостей
     ▼
адаптеры omnibus-adapter-* (подсистема × ОС-семейство; выбор — platform probe):
  a11y:      atspi (Linux)                       / ax (macOS, AXUIElement)
  журнал:    journald                            / unifiedlog
  сервисы:   systemd1                            / launchd
  пакеты:    alpm / apt / dnf                    / brew + mas
  порталы:   xdg-desktop-portal + wl-clipboard   / ScreenCaptureKit + notify
  файлы:     fs-xdg                              / fs-tcc (гранты TCC)
  знания:    kb-archwiki · kb-guides (дистро-доки) / kb-guides (Apple, SS64)
  сеть:      nm read-only «когда есть»            / — (вне v1)
аудит: каждое действие — structured-событие; схема события принадлежит ядру,
       приёмник — сменный AuditSink (journald / unified log / file-fallback)
инференс: в omnibusd ноль модельных вызовов — LLM живёт только в контуре агента,
       выбранного юзером (локальная или облачная — его решение, под его политиками)
```

Ключевые принципы — **read/write асимметрия** (чтение — всегда; запись — только типизированными методами `desktop.set_value` / `desktop.invoke`, не эмуляцией клавиатуры (эмуляция ввода — вне v1; opt-in-расширение — только при доказанной потребности)) и **`capabilities()` как единственный источник истины**: агент не «знает дистрибутив» — он читает машинный отчёт о возможностях; инструмент с отсутствующей capability скрывается из tool list, деградировавший — честно помечает ограничения в схеме, ниже version floor адаптер не активируется. Урок из prior art: скриншот+клик ломается на HiDPI и мультираскладках (`:` → `^` на ru-группе), а чтение дерева даёт ответ в ~300 байт против ~1 МБ скриншота.

**Матрица платформ v1** (полная — §6 архитектуры; здесь — пакетный и журнальный слои как главные источники различий):

| Платформа | Пакеты (`pkg_list`/`pkg_install`/`pkg_remove`) | Журнал (`journal_query`) | Tier |
|---|---|---|---|
| Arch | alpm — `pkg.undo`: downgrade из кэша | persistent из коробки | **T1** |
| Manjaro | alpm (задержанные репы, mhwd/pamac) | persistent ⚠M0 | **T1** |
| Debian trixie / bookworm / bullseye | apt (**undo нет**) | дефолт; включает doctor | **T2 / T2 / T3** |
| Ubuntu LTS | apt (**undo нет**); NM за netplan | volatile по умолчанию; включает doctor | **T2** |
| Mint | apt (**undo нет**) | volatile (унаследовано от Ubuntu) | **T2** |
| Fedora | dnf — `pkg.undo`: transactional | дефолт ⚠M0 | **T2** |
| macOS 14+ | brew + mas | unifiedlog | **T2** |

Version floors: **systemd ≥ 252** (bookworm — на полу; bullseye = 247, ниже пола, потому T3), **macOS ≥ 14** (рекомендуемый 15+ — учтён ежемесячный re-grant права Screen Recording в doctor). Сетевой слой: NM read-only «когда есть», сетевые write-действия вне v1. T1 = гарантированный контракт (полный CI), T2 = полное чтение + безопасная запись (деградации задокументированы), T3 = community без гарантий; платформенные особенности (своя тест-матрица Manjaro, bookworm «на полу», dnf-undo Fedora) — примечания матрицы и `capabilities()`, не отдельные категории; tier — категория обещаний и тестирования, `capabilities()` — машинная правда конкретной машины (§7 архитектуры).

## 3. Rust-стек

`rmcp` ([официальный SDK MCP](https://github.com/modelcontextprotocol/rust-sdk), tokio, rmcp-macros) — в ядре. Граница — крейт `omnibus-contracts`: трейты SPI, DTO, константы capabilities, error taxonomy (`CapabilityRequired`, `ScopeDenied`, `FloorUnsupported`, `AdapterUnavailable`, `NativeFailure{code}`, `ElementStale`, `TimeoutError`, `Unsupported{reason}`); зависимости — только serde/async-типы. Адаптеры: `zbus` (systemd1; NM read-only) + `atspi` (a11y-дерево; Rust-сервер поверх AT-SPI уже существует — [пример](https://mcpmarket.com/server/atspi), фезибилити подтверждено) + `alpm` (Arch/Manjaro), `apt`/`dnf` (Debian-семейство/Fedora) на Linux; байндинги к AX (objc2), unified log и ServiceManagement на macOS. Один статический бинарник; PKGBUILD (linux-набор адаптеров) + Homebrew formula (macos-набор) — install-time уровень детекции из §7.2 архитектуры.

## 4. Поверхность инструментов (MCP)

Имена и привязка к capabilities — по §5 архитектуры; «[+scope]» = требуется write-скоуп из `/etc/omnibus/scopes.d/*.toml` (пример манифеста — §3.3 архитектуры).

- **desktop**: `desktop.tree`, `desktop.element` (адресация role+name, не пиксели), `desktop.set_value` [+scope], `desktop.invoke` [+scope]
- **порталы**: `clip_read` (wl-clipboard/xclip — авто-детект), `clip_write` [+scope `clip.write`], `screenshot` (portal/X11; ScreenCaptureKit на macOS), `notify` [+scope `notify.send`]
- **system**: `svc_list` / `svc_status`, `svc_start` / `svc_stop` [+scope], `net_status` (NM read-only «когда есть»; за netplan/networkd — скрыт), `journal_query` (прошлые загрузки — при `journal.prev_boot`: факт прошлых загрузок, независим от `journal.persistent`; Arch — persistent из коробки; Ubuntu/Mint — opt-in персистентности через doctor, затем перезагрузка)
- **packages**: `pkg_list` / `pkg_info`, `pkg_install` / `pkg_remove` [+scope], `pkg_undo` [+scope; скрыт при `pkg.undo=none`]; откат — только при `pkg.undo` (alpm: downgrade из кэша; dnf: transactional; **у apt нет транзакционного undo** — инструмент честно не обещает откат)
- **files**: `fs_list` / `fs_read` (дефолтный read-скоуп на Linux — $HOME целиком; за пределами $HOME — по явному скоупу; на macOS — TCC-гранты), `fs_write` [+scope]
- **knowledge**: `wiki_search` / `wiki_page` (kb-archwiki на Arch/Manjaro; дистро-доки и kb-guides — на остальных платформах)
- **intents (macOS, MS3)**: `intent_list`, `intent_run` [+scope] — Shortcuts/App Intents (cooperating-приложения; скрыты, пока probe не отчитается о наличии actions)
- **meta**: `capabilities()` (единственный источник истины), `session_info`, `doctor` (ядро; оркеструет probe, журнал, сервисы и базу знаний; через MCP — только диагноз и copy-paste-ремедиация, мутации системы — только в CLI-режиме `omnibus doctor` при исполнении человеком; ведёт через opt-in: персистентность journald, a11y-флаги Electron, TCC-гранты macOS)

## 5. Roadmap

- **M0 (3–4 нед)**: крейт `omnibus-contracts` + platform probe; каркас omnibusd (rmcp); адаптеры atspi + journald + alpm (Arch T1): read-путь `desktop.tree`, `journal_query`, `pkg_list` / `pkg_info`; скоуп-манифест (TOML); PKGBUILD; smoke-тест в виртуальной сессии (Xvfb + headless compositor).
- **M1 (6–8 нед)**: write-пути для X11 и Wayland (typed + clipboard); сервисы (systemd1): `svc_start` / `svc_stop`; аудит (AuditSink → journald); `doctor`; CI с contract test suite на Arch.
- **M2 (~3 мес)**: адаптеры apt (Debian trixie/bookworm, Ubuntu LTS, Mint) и dnf (Fedora) → T2-гарантии; kb-guides: дистро-доки (Debian wiki, Ubuntu docs, Mint docs, Fedora docs) через тот же контракт `KnowledgeBase`; AUR как канал community-провайдеров.
- **M3 (macOS T2)**: адаптеры ax / unifiedlog / launchd / brew / fs-tcc; doctor-визард TCC-грантов (Accessibility, Screen Recording, Automation, Full Disk Access); Homebrew formula + нотаризация.

## 6. Риски

Electron-приложения без a11y-флага (нужен per-app enable); гонка с вендорскими серверами (RHEL Lightspeed уже движется — но их фокус SSH/админ, не десктоп; Apple строит канал для Siri, а не для «bring your own agent»); фрагментация дефолтов дистрибутивов (volatile journald на Ubuntu/Mint — диагностика прошлых загрузок только после opt-in через doctor); TCC-трение macOS (права выдаёт человек через GUI, программно — только MDM/PPPC; Screen Recording в 15+ пере-подтверждается ежемесячно); QA-матрица 7 платформ × DE × X11/Wayland — закрывается tier-моделью (T1 — полный CI, T2 — smoke на эталонных версиях, T3 — community); скриншот на Linux гейтится только порталом DE (Wayland) или ничем (X11) — принятый риск v1 для read-операции (на macOS — TCC).

## 7. Сознательно не строим

Ядра ОС (Linux и macOS), fork компоситора, consent-брокер с UI — никогда (согласие принимается только вне контура агента: TOML-конфиг, `omnibus grant`, `omnibus doctor` (CLI — исполняет человек; doctor через MCP мутировать не может); in-band «пользователь подтвердил» от агента неотличим от подделки — см. §7.4 архитектуры), собственный формат плагинов (MCP есть стандарт — не изобретаем), сетевые write-действия (NM read-only; смена сетевого бэкенда и бэкпорты systemd — никогда, §7.4 архитектуры), модельные вызовы из omnibusd — никогда (инференс целиком в контуре агента юзера; local-ML в будущем — максимум embeddings для поиска, опциональным адаптером, не ядром), событийный канал a11y (и push, и буферизованный поллинг) — вне v1: наблюдение UI = опрос `desktop.tree`, watch-сценарии = `journal_query`/`svc_status`; вернётся аддитивно при первом сценарии, которому опроса мало (push зависит от разнородной поддержки подписок в MCP-клиентах), постоянный демон, admin-канал и HTTP-транспорт — вне v1 (stdio-child покрывает целевые сценарии; пересмотр — при серверных/headless-профилях).

## Приложение: общая карта контекста

- **Концепты**: [AIOS](https://arxiv.org/abs/2403.16971) (агентное ядро), [MemOS](https://arxiv.org/abs/2507.03724) (память как ресурс), [OSWorld 2.0](https://osworld-v2.xlang.ai/) (бенчмарк агентов), «LLM OS» Карпаты (ноя 2023, из памяти)
- **Стандарты**: [MCP](https://modelcontextprotocol.io/docs/getting-started/intro), [ACP](https://agentclientprotocol.com/), [AGENTS.md](https://agents.md/) (60k+ проектов), [llms.txt v2](https://llmstxt.org/), [AAIF](https://aaif.io/)
- **Инструменты**: [Goose](https://goose-docs.ai/) (open-source агент, security-слой), Ollama/llama.cpp, khoj, Letta (из памяти)
- **Прецеденты «AI-дистро»**: [Linux-AI OS](https://github.com/Linux-AI-OS/Linux-AI) (LMDE+модели — бандлинг без архитектуры), [Voxa](https://voxaai.me/), [Meera](https://github.com/achinivar/meera)
