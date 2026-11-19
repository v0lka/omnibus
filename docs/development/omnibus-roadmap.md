# Omnibus — роадмап: MVP → v1.0 → v1.5 → v2.0

**Версия:** 1.0-draft · **Дата:** 2026-10-09 · **Статус:** план работ, companion к [omnibus-spec.md](omnibus-spec.md) (продуктовый слой), [omnibus-architecture.md](omnibus-architecture.md) v1.0 (SPI, матрица платформ, политика деградации) и [omnibus-pitch.md](omnibus-pitch.md) (задачи агента, метрики, каналы)

Этот документ заменяет раздел «Roadmap» спеки (M0–M3) развёрнутой сеткой работ. Продуктовые решения и имена не меняются — все задачи ссылаются на фиксированные сущности архитектуры: крейты (`omnibus-contracts`, `omnibus-core`/`omnibusd`, `omnibus-probe`, `omnibus-adapter-*`), 14 трейтов SPI (12 подсистем §4; реестр закрыт списком задач этого роадмапа — ADR-011), инструменты §5, capabilities §7.1, лестницу деградации §7.3, матрицу платформ §6.

---

## 0. Правила игры

### 0.1. Майлстоун = shippable

Каждый майлстоун заканчивается установленным с штатного пакетного менеджера продуктом, на котором работают killer-демо из питча, — а не «веткой с прогрессом».

| Майлстоун | Результат | Целевые платформы (§6) | Оценка* |
|---|---|---|---|
| **MS1 — MVP** | Arch **T1**: полный контракт (read+write, prev_boot из коробки, `pkg.undo`=cache-downgrade); SPI слоя MS1, probe-архитектура, capabilities-гейтинг | Arch | 10–14 нед |
| **MS2 — v1.0** | + Manjaro **T1** (своя тест-матрица), Debian trixie **T2** / bookworm **T2** (на полу), Ubuntu LTS **T2**, Mint **T2**, Fedora **T2**, Debian bullseye **T3** (сборка есть, гарантий нет) | 8 Linux-платформ | 6–8 нед |
| **MS2.5 — v1.5** | + комплементарные подсистемы Linux (решения D1–D3): запуск приложений (`app.list`/`app.launch`/`app.open` за грантом), процессы и телеметрия (`ps.list`/`ps.info`/`sys.stats`/`power.status`; `ps.signal` за грантом), read-only инвентарь flatpak/snap в подсистеме packages; замкнутый агентный цикл на Linux | те же 8 Linux-платформ (macOS — вне v1.5) | 5–7 нед |
| **MS3 — v2.0** | + macOS **T2** (≥14): адаптеры ax / unifiedlog / launchd / brew / fs-tcc, TCC-визард, нотаризованная формула | 9 платформ | 8–12 нед |

\* Оценки в неделях — для календарного планирования, не обещания; уточняются после Ф1.1 (закрытие ⚠M0-вопросов §10 архитектуры).

### 0.2. Сквозные правила (для каждой задачи)

- **R1. Контракт раньше кода.** Инструмент появляется в tool list только после того, как объявлены его capability-константы и трейт в `omnibus-contracts`, а контракт test suite умеет проверять реализацию.
- **R2. Contract test suite — gate.** Каждая реализация трейта принимается в репозиторий только при зелёном поведенческом наборе против неё (§8.8.1 архитектуры). Это механическая гарантия «новый адаптер — без изменений ядра».
- **R3. Деградация — по лестнице §7.3.** Каждый write-инструмент и каждая платформозависимая фича обязаны определить поведение во всех четырёх состояниях: degraded / hidden (`CapabilityRequired`) / below-floor (`FloorUnsupported`) / opt-in-required.
- **R4. Негативные проверки в CI.** Запреты границы (адаптер ↮ адаптер, адаптер ↮ `omnibus_core::internal`, литералы имён capabilities, tuple-сигнатуры трейтов) проверяются автоматически — PR с нарушением не собирается (§8.8.2).
- **R5. Аудит тотален (P5).** Ни один write-путь не считается готовым без structured-события в AuditSink: `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}`.
- **R6. Никогда-список §7.4 (единый источник — §7.4 архитектуры; сверять по всем пунктам):** write вне грантов, выданных человеком вне контура агента; модельные вызовы из omnibusd; consent-брокер с UI и любые in-band подтверждения; смена сетевого бэкенда; бэкпорты systemd; изменение системных политик безопасности (программная выдача TCC). Не планируются ни в одной задаче; появление такой задачи в ревью = отклонение; тест 1.7.1 «по списку» сверяется с §7.4.
- **R7. Честность продукта.** Формулировки AC не должны обещать больше, чем tier платформы: у apt — «undo нет», на volatile-журнале — prev_boot после opt-in, у недееспособного адаптера — инструмент скрыт.

### 0.3. Формат атомарной задачи

Каждая задача: **What** (что делаем) · **Where** (крейт/файл/путь) · **How** (как, с опорой на архитектуру) · **AC** (Acceptance Criteria — проверяемые критерии, чекбоксы).

---

## 1. MS1 — MVP: Arch Linux T1, полный контракт

**Цель:** на чистом Arch (GNOME/KDE, X11 и Wayland) — полный набор инструментов §5 поверх адаптеров atspi / journald / systemd1 / alpm / xdg-portals / fs-xdg / kb-archwiki; аудит в journald; `doctor` ведёт онбординг. Одновременно закладываются: SPI контрактного слоя MS1 (новые трейты объявляются just-in-time — вместе со своими первыми реализациями), probe-архитектура, capabilities-гейтинг, semver контрактов, негативные CI-проверки. К концу MS1 работают демо №1–3 питча («Диагноз за минуту», «Формы», «После обновления»).

### Ф1.1. Гейт предположений и скелет проекта

Закрывает открытые вопросы §10 (⚠M0) в объёме Arch/Manjaro и создаёт каркас, в котором дальше невозможна деградация дисциплины.

**Задача 1.1.1 — Верификация ⚠M0-допущений (Linux-часть)**
- **What:** фактическая проверка пяти открытых вопросов §10 в объёме, влияющем на MS1–MS2: дефолт персистентности journald на Debian (trixie/bookworm/bullseye), Fedora, Manjaro; фактический пол AT-SPI2 по матрице DE (GNOME/KDE/XFCE/Cinnamon/MATE); systemd-версия актуального Ubuntu LTS (подтвердить 255).
- **Where:** результаты — таблица в `docs/verification/m0-findings.md`; правки полов — §6 архитектуры (следующая минорная ревизия).
- **How:** чистые контейнеры/VM каждого дистрибутива; `ls /var/log/journal`, `systemctl --version`, `gdbus introspect` на AT-SPI2-шине; для AT-SPI2-пола — замер минимальной версии, на которой контрактные операции (tree/element/set_value/invoke) проходят contract test suite на всех пяти DE.
- **AC:**
  - [x] `m0-findings.md` содержит по каждому вопросу факт + источник (версия пакета, вывод команды), а не предположение
  - [x] Дефолты персистентности для 6 дистрибутивов (Manjaro, Debian×3, Fedora, Ubuntu LTS) зафиксированы и перенесены в план Ф2.5 (кому doctor предлагает opt-in)
  - [ ] Пол AT-SPI2 зафиксирован числом и внесён в §6; для каждой DE известно, проходит ли полный контрактный набор
  - [x] systemd-версия Ubuntu LTS подтверждена или исправлена в матрице §6

**Задача 1.1.2 — Cargo workspace и карта крейтов**
- **What:** создать workspace: `omnibus-contracts`, `omnibus-core` (библиотека; бинарь `omnibusd` — отдельный composition-root крейт), `omnibus-probe`, `omnibus-adapter-atspi`, `omnibus-adapter-journald`, `omnibus-adapter-systemd1`, `omnibus-adapter-alpm`, `omnibus-adapter-xdg-portals`, `omnibus-adapter-fs-xdg`, `omnibus-adapter-kb-archwiki`; направление зависимостей — по §2.
- **Where:** корень репозитория, `Cargo.toml` workspace; `specs/decisions/001-dyn-trait-spi.md` — ADR-001 о выборе `dyn Trait` вместо дженериков и динамических плагинов.
- **How:** feature-флаги крейтов адаптеров (compile-time уровень детекции §7.2); правило зависимостей фиксируется в CI на этой задаче, до первого кода адаптеров.
- **AC:**
  - [x] `cargo build --workspace` собирается; `omnibusd --version` отвечает
  - [x] Зависимости: `contracts` не зависит ни от чего кроме serde/async-типов; адаптеры зависят только от `contracts`; core не зависит от адаптеров (адаптеры регистрируются через реестр)
  - [ ] CI-джоба «dependency rules» падает при попытке добавить запрещённое ребро (проверено тестовым PR)

**Задача 1.1.3 — CI-каркас и connascence-линты**
- **What:** конвейер fmt/clippy/test + кастомные линты, кодирующие правила коннасценции §8.5: запрет tuple-сигнатур в трейтах (CoP), запрет нативных типов в `omnibus-contracts`, запрет литералов имён capabilities вне единого реестра (CoN), запрет `omnibus_core` в адаптерах (интрузивная связь).
- **Where:** `.github/workflows/ci.yml` (или аналог), `omnibus-lints/tests/fixtures.rs`, clippy-конфиг.
- **How:** §8.8.2 — линты как закодированная коннасценция; негативные тесты-фикстуры «код, который обязан не собираться».
- **AC:**
  - [x] Каждое из 4 правил подтверждено негативным тестом (фикстура с нарушением красная, исправленная — зелёная)
  - [ ] CI гоняет fmt/clippy/test/lints на каждом PR; время прогона < 10 мин на кэшированном раннере

**Задача 1.1.4 — Contract test suite: harness**
- **What:** поведенческий набор в `omnibus-contracts`: для каждого трейта — обязательный набор сценариев (happy path, ошибки `NativeFailure`, таймауты, пустые ответы), который гоняется против любой реализации; набор растёт just-in-time с объявлением новых трейтов.
- **Where:** `omnibus-contracts/src/suite/` (семейные suite-функции, например `a11y_suite`, связываемые из `tests/contract_suite.rs` адаптеров).
- **How:** §8.6/§8.8.1 — suite как gate; реализации подключают suite своим тест-биндером; фейковые in-memory реализации для тестов ядра живут рядом (не в адаптерах — P6).
- **AC:**
  - [x] Harness компилируется и гоняется против фейковой реализации каждого трейта
  - [x] Документировано: как добавить реализацию и привязать suite (инструкция в README контрактов)
  - [x] CI-джоба запускает suite против всех существующих реализаций; провал = запрет merge

### Ф1.2. omnibus-contracts: полная SPI

**Задача 1.2.1 — SPI контрактного слоя MS1: 10 трейтов (8 подсистем)**
- **What:** объявить подсистемы MS1: `DesktopA11y`, `Journal` + `AuditSink`, `ServiceManager`, `PackageManager`, `ScreenCapture` + `Notify`, `FileScope`, `KnowledgeBase`, `PlatformProbe`. Трейты объявляются **just-in-time** — вместе со своими первыми реализациями: `NetworkStatus` — в Ф2.2.3, `AppLauncher` — в Ф2.7.1 и `ProcessTelemetry` — в Ф2.8.1 (v1.5), `AppIntentsBridge` — в Ф3.6.2 (все — аддитивное расширение контракта, ядро не меняется); заблаговременные «заделы» не строятся — добавление трейта и так аддитивно.
- **Where:** `omnibus-contracts/src/spi/*.rs`.
- **How:** сигнатуры — именованные структуры (никаких tuple-параметров), операции — по колонке «Ключевые операции контракта» §4 (например `PackageManager`: `list`, `info`, `search`, `install`, `remove`, `history`, `undo`).
- **AC:**
  - [x] 10 трейтов (8 подсистем) объявлены — ровно те, у которых в MS1 есть реализация
  - [x] Каждый трейт имеет дескриптор capabilities (какие capability объявляет реализатор)
  - [x] Правила маппинга ошибок: каждый метод возвращает контрактную ошибку, `NativeFailure{code}` сохраняет payload исходного сообщения
  - [x] Версия контракта (semver) проставлена и независима от версии кода

**Задача 1.2.2 — DTO-дисциплина и opaque-токены**
- **What:** плоские именованные DTO для всех операций; newtype-обёртки (`Milliseconds`, `Bytes`, `Severity`); типы адресации: `element_id`, `session_id` — opaque-токены, mintит только ядро; нативные типы (`zbus::Value`, хэндлы ALPM, `AXUIElementRef`) не покидают адаптер.
- **Where:** `omnibus-contracts/src/dto/*.rs`, `omnibus-contracts/src/tokens.rs`.
- **How:** §4 «Дисциплина DTO», §8.5 (CoM→CoT, CoI→opaque-токены).
- **AC:**
  - [x] Ни один DTO не содержит кортежей и native-типов (линт 1.1.3 зелёный)
  - [ ] Токены нельзя сконструировать вне ядра (приватный конструктор, проверено компиляцией негативного примера)
  - [x] Сериализация DTO (serde) — единственное разделяемое поведение; зафиксировано в README контрактов

**Задача 1.2.3 — Реестр capabilities и error taxonomy**
- **What:** константы всех capabilities §7.1 (`desktop.tree`, `desktop.write.typed`, `journal.persistent`, `journal.prev_boot`, `journal.query`, `service.read`, `service.write`, `pkg.read`, `pkg.write`, `pkg.undo` со значениями `none`/`cache-downgrade`/`transactional`, `portal.clipboard`, `portal.screenshot`, `portal.notify`, `files.scope.read`, `files.scope.write`, `files.scope.tcc`, `kb.present`, `net.read`), формат `{domain}.{feature}[.{qualifier}]` + semver + статус (`present`/`degraded`/`opt-in-required`), для полиформенных capability — значение-enum; ошибки: `CapabilityRequired`, `ScopeDenied`, `FloorUnsupported`, `AdapterUnavailable`, `NativeFailure{code}`, `ElementStale`, `TimeoutError`, `Unsupported{reason}`.
- **Where:** `omnibus-contracts/src/capabilities.rs`, `omnibus-contracts/src/errors.rs`.
- **How:** §7.1 — реестр один, литералы в коде запрещены (линт Ф1.1.3); capability не может объявиться вне контракта (CoV).
- **AC:**
  - [x] Все имена capabilities существуют только как константы; grep-линт по репозиторию на литералы зелёный
  - [x] `pkg.undo` типизирован enum'ом значений, а не строкой
  - [x] Error taxonomy покрывает все методы всех трейтов; в contracts нет `anyhow`/`Box<dyn Error>` на границе

### Ф1.3. omnibus-core: ядро omnibusd

**Задача 1.3.1 — MCP-фасад и реестр инструментов**
- **What:** транспорт stdio (rmcp, tokio; HTTP — вне v1); топология stdio-child: инстанс на MCP-соединение (§3.1); реестр инструментов как `(имя, JSON-схема, требуемые capabilities, обработчик)`; генерация схем из DTO; каркас инструментов ядра (doctor — оркестрация трейтов поверх SPI).
- **Where:** `omnibus-core/src/facade.rs`, `omnibus-core/src/registry.rs`.
- **How:** §3.1 и §3.6; правило §5 — инструмент живёт в ядре и вызывает трейт, а не реализацию; ни один инструмент не импортирует нативные крейты.
- **AC:**
  - [ ] `omnibusd` отвечает на MCP-хендшейк по stdio; фиктивный инструмент проходит round-trip из Claude Code/goose
  - [x] JSON-схемы инструментов генерируются из DTO (не написаны руками); расхождение = ошибка компиляции/теста
  - [x] Скрытие инструмента без capabilities работает: инструмент отсутствует в `tools/list`, прямой вызов → `CapabilityRequired` с подсказкой doctor

**Задача 1.3.2 — Сессии**
- **What:** сессия = MCP-соединение (§3.2: живёт и умирает с процессом инстанса, без TTL; идентификатор — для корреляции аудита); mint всех opaque-токенов (session_id, element_id) — только ядром; гранты — единый механизм TOML-манифестов + `omnibusd grant` (§3.3); политика лимитов (дефолты §3.10: tool-call 10 с, tree 15 с, D-Bus 2 с) и ретраи: один авторетрай идемпотентных read, write — никогда.
- **Where:** `omnibus-core/src/session.rs`.
- **How:** §3.2; жизненный цикл адаптера `probe → register → serve → shutdown` — конечный автомат ядра (CoE, §8.5); адаптер физически не вызывается до register.
- **AC:**
  - [x] Таймаут read после одного авторетрая → typed `TimeoutError`; write не ретраится (тест)
  - [x] Параллельные write двух инстансов сериализуются (мьютекс + flock, §3.9): оба завершаются, аудит фиксирует порядок захвата (тест)
  - [x] element_id, выданный одним инстансом, не резолвится другим (typed-ошибка, тест на два последовательных инстанса)
  - [x] Вызов адаптера вне состояния serve невозможен (типизируется автоматом, негативный тест)

**Задача 1.3.3 — Скоупы**
- **What:** TOML-манифесты `/etc/omnibus/scopes.d/*.toml`; read-only по умолчанию; гранты — только выданные человеком вне контура агента, единый механизм: записи в манифестах (руками или `omnibusd grant <грант>` — команда редактирует манифест); все решения о доступе — в ядре.
- **Where:** `omnibus-core/src/scopes.rs`; манифест-шаблоны `packaging/scopes.d/*.toml`.
- **How:** §3.3; скоуп-модель из спеки: «[+scope]» у `desktop.set_value`, `desktop.invoke`, `svc_start`, `svc_stop`, `pkg_install`, `pkg_remove`, `pkg_undo`, `fs_write`, `clip_write`, `notify`; opt-in-гранты — тот же механизм: записи в тех же манифестах (секция `[optin]`, имя = capability в статусе `opt-in-required`, напр. `journal.persistent` на машинах с volatile-конфигом journald); выдача — та же команда `omnibusd grant <имя>`, doctor возвращает готовую команду; ревокация — удалением записи («временный» грант = выданный и отозванный); provenance в аудите тот же (`config|cli|doctor`); инвариант «нет confirm-API» — ни один MCP-метод не принимает токен согласия, у каждого грант-события аудита provenance `config|cli|doctor`.
- **AC:**
  - [x] Write-инструмент без гранта → `ScopeDenied`, событие аудита записано, тест зелёный
  - [x] Инвариант no-confirm-API: негативный тест — «подтверждение» от агента (токен/поле согласия в аргументах) игнорируется, действие отклонено как без гранта
  - [x] Каждое грант-событие аудита имеет provenance `config|cli|doctor` — тест
  - [x] Манифест с синтаксической ошибкой не роняет демон: онбординг-ошибка в `doctor`, остальные скоупы работают
  - [x] Дефолтная установка (без пользовательских томов) даёт ровно read-only набор — проверено интеграционным тестом на свежем манифесте

**Задача 1.3.4 — Аудит-пайплайн**
- **What:** единая схема события `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}`; аудит на уровне фасада (адаптеры о нём не знают); приёмник — трейт `AuditSink`; реализации: journald (основная), file-fallback (тесты и нестандартные конфигурации).
- **Where:** `omnibus-core/src/audit.rs`; `omnibus-adapter-journald` (AuditSink-часть), file-fallback — внутри `omnibus-core/src/audit.rs`.
- **How:** §3.4, P5; params_hash — хэш параметров (не сами параметры — в журнал не попадают чувствительные данные).
- **AC:**
  - [x] Каждое действие любого инструмента (read тоже) даёт ровно одно событие с полной схемой — интеграционный тест по всем инструментам §5
  - [x] Падение AuditSink не роняет инструмент: событие уходит в file-fallback, деградация видна в `capabilities()`/`doctor`
  - [x] `params_hash` обратимо не восстанавливает параметры (ревью + тест на отсутствие сырых параметров в событии)

**Задача 1.3.5 — Реестр capabilities() и гейтинг инструментов**
- **What:** агрегатор отчётов адаптеров в один документ для агента; связка «capability → инструмент» по §5; каждая сессия строит документ из свежего probe-отчёта — изменения конфига/среды подхватываются новой сессией (§7.2).
- **Where:** `omnibus-core/src/caps.rs`.
- **How:** §7.1–§7.3: статусы present/degraded/opt-in-required; четыре состояния деградации лестницы; в спорах capabilities() выигрывает (§7.5).
- **AC:**
  - [x] Документ `capabilities()` содержит провайдера, версию и ограничения каждой capability
  - [x] Интеграционный тест лестницы: (a) degraded — инструмент в списке, схема помечает ограничение; (b) отсутствующая — скрыт, прямой вызов → `CapabilityRequired` с подсказкой doctor; (c) below-floor — адаптер не активирован, документ объясняет причину («systemd 247 < 252»); (d) грант не выдан — статус opt-in-required, поведение = (b)
  - [x] Изменение среды подхватывается новой сессией: инстанс, запущенный после doctor-CLI, видит обновлённый tool list (тест на два последовательных инстанса)

### Ф1.4. omnibus-probe и end-to-end деградация

**Задача 1.4.1 — PlatformProbe для Arch**
- **What:** `detect() → PlatformReport{distro/release, versions, живость демонов}`: `/etc/os-release`, `systemctl --version`, systemd1 на шине, AT-SPI2 в сессии, wl-clipboard/xclip наличие; выбор и активация адаптеров (по одному активному на подсистему — §9).
- **Where:** `omnibus-probe/src/`.
- **How:** §4.8, §7.2 runtime-уровень (перекрывает compile/install-time); отчёт → реестр capabilities (Ф1.3.5).
- **AC:**
  - [ ] На чистом Arch (X11 и Wayland — ось CI; ось DE GNOME/KDE сознательно не вводится в CI, пока ни один шаг не ветвится по типу сессии — критерий её ввода зафиксирован в шапке `.github/workflows/ci.yml`, реальное покрытие DE остаётся за VM-ногами) probe выдаёт корректный отчёт и активирует полный набор адаптеров
  - [x] Отсутствие AT-SPI2 в сессии → `desktop.*` скрыты, документ объясняет причину и ссылается на doctor
  - [x] Новый инстанс после изменения среды перевыбирает адаптеры по свежему probe (тест на перезапуск)

**Задача 1.4.2 — Version floor как механизм**
- **What:** enforcement floor'ов: systemd ≥ 252, AT-SPI2 ≥ пола из Ф1.1.1; ниже пола адаптер не активируется, `capabilities()` объясняет причину.
- **Where:** `omnibus-probe/src/floors.rs`; константы полов — в `omnibus-contracts`.
- **How:** §6 «Version floors», лестница §7.3.3. На Arch floor не срабатывает (rolling) — механизм тестируется интеграционно на симулированном окружении (стаб-отчёт systemd 247) и по-настоящему — в MS2 (bookworm/bullseye).
- **AC:**
  - [x] Тест со стабом «systemd 247» → адаптер не активирован, документ содержит точную причину
  - [x] Пол AT-SPI2 берётся из константы, подтверждённой Ф1.1.1 (не магическое число в коде probe)
  - [x] Документация doctor описывает, что увидит агент ниже пола

### Ф1.5. Адаптеры чтения (Arch)

**Задача 1.5.1 — adapter-atspi: read-путь**
- **What:** `tree(app)`, `element(id)`, `read_attr` поверх AT-SPI2 over D-Bus; адресация role+name (не пиксели); кодеки a11y-дерева в DTO.
- **Where:** `omnibus-adapter-atspi/src/`.
- **How:** крейт `atspi`; обход дерева и разрешение адресов — собственность адаптера (CoA, контракт задаёт только вход/выход); урок prior art — чтение дерева ~300 байт вместо ~1 МБ скриншота.
- **AC:**
  - [ ] Contract suite `DesktopA11y` зелёный против реализации на живом GTK- и Electron-приложении (в CI — Xvfb + headless compositor)
  - [x] `desktop.tree` на эталонном приложении отдаёт дерево с role+name-адресацией; повторный вызов в бюджете времени (тест)

**Задача 1.5.2 — adapter-journald: Journal**
- **What:** `query(Filter{unit, boot, severity, window})`, `tail`; `prev_boot` — независимый признак «факт прошлых загрузок» (не выводится из `journal.persistent`: свежая persistent-машина без перезагрузок ещё не имеет prev_boot).
- **Where:** `omnibus-adapter-journald/src/journal.rs`.
- **How:** journald JSON-интерфейс; `journal.prev_boot` объявляется при факте прошлых загрузок в журнале; на Arch persistent из коробки, prev_boot входит в T1-контракт (при наличии прошлых загрузок).
- **AC:**
  - [x] Contract suite `Journal` зелёный; фильтры unit/severity/window проверены на синтетическом журнале
  - [ ] `journal_query` с prev_boot возвращает события предыдущей загрузки на persistent-машине; на volatile-стенде `journal.prev_boot` имеет статус `opt-in-required` (тест на двух стендах)

**Задача 1.5.3 — adapter-systemd1: чтение**
- **What:** `list`, `status` через org.freedesktop.systemd1 (zbus).
- **Where:** `omnibus-adapter-systemd1/src/`.
- **How:** read-only методы; юниты в DTO; коды ошибок D-Bus → `NativeFailure{code}` с сохранением payload.
- **AC:**
  - [x] Contract suite `ServiceManager` (read-часть) зелёный; `svc_list`/`svc_status` на 100+ юнитах системы не деградируют по времени (бюджет в тесте)
  - [x] Несуществующий юнит → контрактная ошибка, а не паника/необработанный D-Bus-ответ

**Задача 1.5.4 — adapter-alpm: чтение**
- **What:** `list`, `info`, `search`, `history` через alpm.
- **Where:** `omnibus-adapter-alpm/src/`.
- **How:** крейт `alpm`; хэндлы ALPM не покидают адаптер; `history` из pacman-лога — основа будущего `undo`.
- **AC:**
  - [x] Contract suite `PackageManager` (read-часть) зелёный; `pkg_list` на системе с 1000+ пакетов в бюджете времени
  - [x] `pkg_info` возвращает метаданные установленного и доступного пакета; `history` читает транзакции pacman-лога

**Задача 1.5.5 — adapter-xdg-portals: clipboard (read+write), screenshot, notify**
- **What:** `clip_read`; `clip_write` [+scope — write-действие, несмотря на группировку «порталы»]; `screenshot` (portal/X11); `notify` [+scope] (D-Bus notifications).
- **Where:** `omnibus-adapter-xdg-portals/src/`.
- **How:** xdg-desktop-portal + wl-clipboard; capability `portal.clipboard`/`portal.screenshot`/`portal.notify` объявляются по факту наличия бэкенда (авто-детект — часть probe-отчёта).
- **AC:**
  - [ ] На GNOME и KDE (X11 + Wayland): round-trip clipboard, скриншот, уведомление — интеграционные тесты в виртуальной сессии
  - [ ] Отсутствие бэкенда (стенд без wl-clipboard) → соответствующие инструменты скрыты, документ объясняет

**Задача 1.5.6 — adapter-fs-xdg и adapter-kb-archwiki: чтение**
- **What:** `fs_list`/`fs_read` в грантированных XDG-скоупах; `wiki_search`/`wiki_page` по ArchWiki.
- **Where:** `omnibus-adapter-fs-xdg/src/`, `omnibus-adapter-kb-archwiki/src/`.
- **How:** дефолтный read-скоуп — $HOME целиком, включая dotfiles (решение 2026-10-10); за пределами $HOME — только по явному скоупу; `fs_write` — только по гранту (запрет путей — 1.6.4); KB-адаптер кэширует статьи (свежесть rolling); применимость статьи оценивает агент (P7).
- **AC:**
  - [x] `fs_read` за пределами гранта → `ScopeDenied` (тест); внутри `~/Documents` — читает, бюджет и лимиты размеров зафиксированы
  - [x] `wiki_search` находит статью по запросу «suspend» и отдаёт `wiki_page` с содержимым; офлайн-режим деградирует честно (stale-кэш помечен)

### Ф1.6. Адаптеры записи (Arch)

**Задача 1.6.1 — adapter-atspi: write-путь (typed)**
- **What:** `set_value`, `invoke` через typed-интерфейсы AT-SPI2; без эмуляции клавиатуры.
- **Where:** `omnibus-adapter-atspi/src/lib.rs`.
- **How:** P2/P3 — запись только типизированными методами; требует `desktop.write.typed` + write-скоуп; раскладко-независимость по построению (урок `:` → `^` на ru-группе — в тест-матрице).
- **AC:**
  - [ ] Contract suite `DesktopA11y` (write-часть) зелёный: set_value на эталонной форме меняет значение, invoke нажимает кнопку — на X11 и Wayland, при активной ru-раскладке включён отдельный тест
  - [x] Запись без write-скоупа → `ScopeDenied` + аудит-событие
  - [ ] HiDPI-стенд: операции адресуются по дереву, координаты нигде не фигурируют (код-ревью + тест)

**Задача 1.6.2 — adapter-systemd1: write**
- **What:** `start/stop/restart`, `enable/disable` (+`svc_start`/`svc_stop`, write-скоуп, `service.write`).
- **Where:** `omnibus-adapter-systemd1/src/lib.rs`.
- **How:** polkit-права системы действуют как есть (мы не перехватываем авторизацию); ошибки авторизации → честная контрактная ошибка.
- **AC:**
  - [ ] Contract suite (write-часть) зелёный: рестарт тестового юнита доходит до активного состояния (таймаут-политика ядра)
  - [x] Отказ polkit → `NativeFailure` с человекочитаемой подсказкой; аудит-событие с результатом отказа записано

**Задача 1.6.3 — adapter-alpm: write + undo**
- **What:** `install`, `remove` (+`pkg_install`/`pkg_remove`, write-скоуп); `undo` = downgrade из кэша → `pkg.undo=cache-downgrade`; MCP-инструмент `pkg_undo` [+scope] регистрируется в ядре (скрыт при `pkg.undo=none`).
- **Where:** `omnibus-adapter-alpm/src/lib.rs`.
- **How:** undo восстанавливает предыдущую версию из pacman-кэша по `history`; если версии нет в кэше — честная деградация (подсказка: ALA/AUR Archive в документации KB, не в коде адаптера — P7).
- **AC:**
  - [ ] Contract suite (write-часть) зелёный: install/remove идемпотентны к повтору, ошибки транзакций → `NativeFailure`
  - [ ] Сценарий «обновил → сломалось → undo»: downgrade из кэша возвращает предыдущую версию (интеграционный тест с фейковым пакетом)
  - [x] Нет версии в кэше → `NativeFailure` с объяснением; capabilities не обещают того, чего нет

**Задача 1.6.4 — fs-xdg: write + сквозной аудит (AuditSink)**
- **What:** `fs_write` в грантированных скоупах; проверка, что write-события всех адаптеров проходят через единый аудит-пайплайн ядра.
- **Where:** `omnibus-adapter-fs-xdg/src/lib.rs`; сквозной тест в `omnibus-core/tests/scopes_grants.rs`.
- **How:** P5/P6 — адаптер не знает об аудите; ядро оборачивает каждый вызов.
- **AC:**
  - [x] Сквозной тест: write через atspi/systemd1/alpm/fs-xdg → по одному событию в AuditSink с корректными tool/adapter/op
  - [x] `fs_write` за пределами гранта → `ScopeDenied`; внутри — пишет, лимиты и запрет путей (никогда не записываемые префиксы /etc, /usr, /boot, /proc, /sys, /dev, /var + грант-стор и state-каталог демона; обычные dotfiles записываемы) зафиксированы в скоуп-модели и тестах

### Ф1.7. doctor

**Задача 1.7.1 — doctor**
- **What:** probe + журнал + сервисы + KB; диагноз предпосылок самого Omnibus; ведёт через opt-in: a11y-флаги Electron-приложений, (на Arch — как путь на будущее) включение персистентности journald; изменения подхватываются новой сессией (§7.2). Разделение каналов: вызов через MCP от агента — только диагноз + copy-paste-ремедиация (как у `ScopeDenied`); мутации системы — только в CLI-режиме `omnibusd doctor`, исполняемом человеком.
- **Where:** `omnibus-core/src/doctor.rs` (оркестрация трейтов — обязанность ядра, P6).
- **How:** §7.4 — opt-in только с явным согласием, собираемым вне контура агента (мутацию исполняет человек — CLI или выданная им команда); никогда-список соблюдён; TTFM-путь: `pacman -S omnibus` → `doctor` → одна строка в конфиг MCP-клиента.
- **AC:**
  - [ ] На свежей Arch-VM с GNOME/KDE: doctor находит отсутствующие предпосылки, ведёт через включение, повторный запуск — зелёный; TTFM < 5 минут (замер в тесте онбординга)
  - [x] Каждое doctor-изменение системы — отдельное аудит-событие; согласие собирается вне контура агента (CLI, исполняет человек) — granular opt-in, не «починить всё»
  - [x] Doctor через MCP не мутирует систему ни при каком ответе агента: результат — диагноз + готовая команда для человека (тест с «согласившимся» агентом)
  - [x] Doctor не предлагает ничего из никогда-списка §7.4 (тест по списку)

### Ф1.8. Упаковка, QA-гейты и релиз MVP

**Задача 1.8.1 — PKGBUILD и AUR**
- **What:** PKGBUILD (linux-набор адаптеров — install-time детекция §7.2) + публикация в AUR.
- **Where:** `packaging/PKGBUILD`; AUR-репозиторий.
- **How:** один статический бинарник; манифесты скоупов ставятся в `/etc/omnibus/scopes.d/`.
- **AC:**
  - [ ] `pacman -S omnibus` (из AUR) на чистой Arch-VM → рабочий демон, TTFM < 5 мин
  - [ ] Обновление версии проходит без ручных шагов (тест обновления с предыдущей версии)

**Задача 1.8.2 — CI: виртуальные сессии и полный гейт T1**
- **What:** матрица CI: X11/Wayland на Arch-контейнерах (ось DE GNOME/KDE сознательно не вводится, пока ни один шаг CI не ветвится по типу сессии — критерий её ввода зафиксирован в шапке `.github/workflows/ci.yml`; реальное покрытие DE — VM-ноги); Xvfb + headless compositor; contract suite против всех реализаций; негативные проверки зависимостей; smoke демо №1–3.
- **Where:** `.github/workflows/`; `ci/smoke/`.
- **How:** T1 = полный CI на каждый релиз ядра и адаптеров (§6); headless GUI-тесты — одновременно фича для разработчиков (питч, кластер E).
- **AC:**
  - [ ] Демо №1 («Диагноз за минуту»: journal_query(prev_boot, unit) → диагноз → план фикса из статьи KB → svc_start по гранту из doctor → верификация: svc_status active + чистый tail журнала) проходит как автотест
  - [ ] Демо №2 («Формы»: fs_read → desktop.tree → set_value → invoke) проходит как автотест
  - [ ] Демо №3 («После обновления»: `pkg_undo` — downgrade из кэша) проходит как автотест
  - [ ] Полный прогон матрицы зелёный; красный main невозможен по правилам репозитория

**Задача 1.8.3 — Релиз MVP**
- **What:** tagged-релиз, заметки с честной матрицей (Arch T1 — единственная гарантированная платформа), публикация: Show HN + r/archlinux (разбор архитектуры), доклад-заявка на a11y-трек GNOME/KDE.
- **Where:** GitHub Releases; каналы по питчу (AUR → Show HN → пресса).
- **How:** метрики: TTFM замеряется автотестом онбординга (Ф1.8.2), north star — успешные агентные задачи в неделю (опрос/ручной трекинг без телеметрии).
- **AC:**
  - [x] Релизные заметки содержат:capabilities-документ эталонной машины, известные ограничения, never-список
  - [x] Никакой обязательной телеметрии (ревью кода: отсутствие сетевых отправок без явного opt-in)
  - [ ] DoD MS1: демо 1–3 зелёные в CI, TTFM < 5 мин, полный contract suite, негативные проверки активны

---

## 2. MS2 — v1.0: все целевые дистрибутивы Linux

**Цель:** матрица §6 по Linux закрыта: Manjaro T1 (своя тест-матрица), Debian trixie T2 / bookworm T2 (systemd 252 на полу), Ubuntu LTS T2, Mint T2, Fedora T2, Debian bullseye T3 (ниже пола systemd 247 — сборка есть, гарантий нет). Демонстрация ключевого свойства архитектуры: **ядро не меняется** — добавляются только адаптеры apt/dnf, kb-guides, ветки probe. К концу MS2 работает демо №4 («С нуля»: пустая VM → машина под задачу по базе знаний платформы).

### Ф2.1. Probe-обобщение: семейства и полы

**Задача 2.1.1 — Дистро-детекция и выбор семейства адаптеров**
- **What:** расширение `PlatformReport`: Manjaro (arch-семейство, но отдельная ветка), Debian/Ubuntu/Mint (apt-семейство), Fedora (dnf); выбор пакетного адаптера по семейству при старте инстанса.
- **Where:** `omnibus-probe/src/detect.rs`.
- **How:** `/etc/os-release` + факт наличия пакетных бэкендов; одно активное семейство на подсистему (§9); при нескольких бэкендах приоритет нативного семейства — задокументированное правило; Manjaro ≠ Arch: задержанные репы, mhwd, pamac — в отчёте probe.
- **AC:**
  - [ ] На 6 целевых дистрибутивах (VM-матрица CI) probe выбирает корректный набор адаптеров
  - [x] Документ probe-отчёта публикуется в capabilities (агент видит distro/release/versions)

**Задача 2.1.2 — Полы на реальных дистрибутивах**
- **What:** enforcement systemd ≥ 252: bookworm (252 — на полу) активен; bullseye (247) — адаптер не активируется; подтверждение флага `journal.persistent` по фактам Ф1.1.1 (persistent — факт конкретной машины; свежие установки по M0 табл. 1 персистентны во всех семьях).
- **Where:** `omnibus-probe/src/floors.rs`; тесты на VM-матрице.
- **How:** лестница §7.3.3; в спорах capabilities() — машинная правда.
- **AC:**
  - [ ] bullseye-VM: инструменты семейства pkg скрыты, `capabilities()` объясняет «systemd 247 < 252», демон работает (чтение journal/desktop, если полы пройдены)
  - [ ] bookworm-VM: полный T2-набор активен; автотест фиксирует «на полу» статус (любой даунгрейд systemd в релизе — тест падает и сигналит пересмотр)
  - [ ] Ubuntu/Mint-VM: `journal.persistent=false` в документе capabilities; prev_boot скрыт

### Ф2.2. adapter-apt (Debian trixie / bookworm, Ubuntu LTS, Mint)

**Задача 2.2.1 — PackageManager на apt: чтение**
- **What:** `list`, `info`, `search`, `history` через apt (libapt или CLI-маппинг — решение ADR-002).
- **Where:** `omnibus-adapter-apt/src/`.
- **How:** history из `/var/log/apt/history.log`; контракт тот же, что у alpm — suite без изменений (R2).
- **AC:**
  - [ ] Contract suite `PackageManager` зелёный на trixie и bookworm; на Ubuntu LTS и Mint — те же тесты в VM-матрице
  - [ ] `pkg_list`/`pkg_info` по бюджету времени на системе с ~2000 пакетов

**Задача 2.2.2 — apt: write и честное «undo нет»**
- **What:** `install`, `remove`; `undo` не реализуется → `pkg.undo=none`; схема `pkg_install` помечает отсутствие отката (degraded-состояние §7.3.1).
- **Where:** `omnibus-adapter-apt/src/write.rs`; схема инструмента в ядре (читает capability).
- **How:** лестница §7.3.1 — инструмент присутствует, ограничение видно агенту; демо №3 на apt показывает деградацию «журнал + план ручного фикса» (питч).
- **AC:**
  - [x] JSON-схема pkg_install/pkg_remove на apt-машине содержит явное «undo недоступен (apt не имеет транзакционного отката)»
  - [x] Попытка undo → `CapabilityRequired` с подсказкой (а не ошибка выполнения посреди транзакции)
  - [x] Транзакции apt не требуют интерактивного ввода (CLI-флаги зафиксированы в тесте на non-tty)

**Задача 2.2.3 — NM read-only «когда есть»**
- **What:** read-only сетевые данные через NetworkManager D-Bus при наличии: инструмент `net_status`, capability `net.read` (устройства, connectivity, wifi-сети); за netplan/networkd (Ubuntu Server) — capability отсутствует, инструмент скрыт; write-действия на сеть — никогда (§7.4).
- **Where:** отдельный `omnibus-adapter-nm` (ADR-003 закрыт: NM — отдельный D-Bus-сервис и подсистема §4.10; трейт `NetworkStatus` объявляется здесь, в `omnibus-contracts`, вместе с первой реализацией — just-in-time).
- **How:** §6 «NM read-only»; contract suite `NetworkStatus` (Ф1.1.4) зелёный против реализации; отсутствие NM не влияет на остальные подсистемы.
- **AC:**
  - [ ] На desktop-стендах с NM сетевой статус виден; на networkd-стенде инструменты сети скрыты без ошибок в журнале
  - [x] В коде адаптера отсутствуют write-методы NM (ревью + контракт не объявляет их)
  - [ ] `net_status` в tool list только при живом NM; прямой вызов на стенде без NM → `CapabilityRequired`

### Ф2.3. adapter-dnf (Fedora, T2)

**Задача 2.3.1 — PackageManager на dnf + transactional undo**
- **What:** `list`, `info`, `search`, `install`, `remove`, `history`, `undo` через dnf history undo/rollback → `pkg.undo=transactional`.
- **Where:** `omnibus-adapter-dnf/src/`.
- **How:** dnf/dnf5 — замена провайдера без изменения контракта (обратный сценарий §8.6): сигнатуры и capabilities не меняются.
- **AC:**
  - [ ] Contract suite зелёный на Fedora; undo возвращает систему к предыдущей transaction-точке (интеграционный тест)
  - [ ] `pkg.undo=transactional` отражён в capabilities; демо №3 проходит на Fedora в CI
  - [ ] Свежие portal-интерфейсы Fedora: screenshot/clipboard работают (smoke на актуальном release)

### Ф2.4. Manjaro T1 (своя тест-матрица)

**Задача 2.4.1 — Manjaro-ветка: контракт T1 + своя тестовая матрица**
- **What:** alpm/journald/systemd1/atspi работают как на Arch; различия: задержанные репы, mhwd-драйверы, pamac; journald-persistent по факту Ф1.1.1 (⚠M0); отдельная CI-матрица.
- **Where:** `omnibus-probe` (ветка Manjaro), `packaging/` (PKGBUILD-вариант или собственный пакет), CI.
- **How:** Manjaro = T1; дистро-специфика (mhwd, pamac, задержанные репы) — примечания матрицы §6 и отчёт probe; kb — ArchWiki + Manjaro-overlay (Ф2.5).
- **AC:**
  - [ ] Полный T1-набор автотестов зелёный на Manjaro-VM (GNOME/KDE × X11/Wayland)
  - [ ] mhwd-специфика (видеодрайверы) не ломает desktop-подсистему (smoke на VM с non-free драйвером)
  - [x] Задержка реп обнаружена probe'ом и отражена в отчёте (не в тишине)

### Ф2.5. doctor multi-distro и kb-guides

**Задача 2.5.1 — doctor: opt-in персистентности journald**
- **What:** на машинах, где зонд сообщил volatile-конфиг journald (по фактам Ф1.1.1), doctor предлагает включить persistent (`mkdir /var/log/journal` + restart journald) с явным согласием вне контура агента (мутацию исполняет человек — CLI-режим doctor или выданная им команда); следующий инстанс после doctor-CLI видит `journal.persistent=true`; `journal.prev_boot` появляется после следующей перезагрузки (факт прошлых загрузок — независимый признак).
- **Where:** `omnibus-core/src/doctor.rs`; `omnibus-probe` (перечитывание при старте инстанса).
- **How:** §7.4 opt-in-список; флагманский сценарий «ноутбук не проснулся» становится доступен на T2 после opt-in — задокументировать как пример деградационной лестницы §7.3.4 → §7.3.1.
- **AC:**
  - [ ] Ubuntu-VM с намеренно volatile-конфигом journald (убран /var/log/journal): до opt-in `journal.persistent` = `opt-in-required`, prev_boot скрыт; после гранта и ручного создания персистентного хранилища инстанс, запущенный затем, видит persistent=true; после перезагрузки (вторая загрузка стенда в автотесте) prev_boot=true и журнал прошлой загрузки читается
  - [x] Согласие отдельное, гранулярное (не пакетом с другими opt-in), собирается вне контура агента; каждое изменение — аудит-событие
  - [x] Отказ от opt-in оставляет систему в read-only-статус-кво без повторных навязчивых предложений (политика частоты — тест)

**Задача 2.5.2 — adapter-kb-guides: дистро-доки**
- **What:** `KnowledgeBase` на дистро-доках (Debian wiki, Ubuntu docs, Mint docs/forums, Fedora docs); ArchWiki+Manjaro-overlay — через существующий kb-archwiki. Плейбук-машинерии нет: применимость статьи оценивает агент (у него есть `capabilities()` и контекст задачи — P7).
- **Where:** `omnibus-adapter-kb-guides/src/`.
- **How:** P7 — диагноз универсален, лечение дистро-осознанно; тот же контракт `KnowledgeBase`, что у kb-archwiki (suite без изменений, R2); кэш статей как в Ф1.5.6.
- **AC:**
  - [ ] `wiki_search` на Ubuntu находит fix по known-issue через дистро-доки; `wiki_page` отдаёт статью; офлайн-режим деградирует честно (stale-кэш помечен)
  - [ ] Тот же диагноз (suspend-провал) на Arch и Ubuntu ведёт к разным статьям — интеграционный тест пары машин

### Ф2.6. Каналы, QA-матрица, релиз v1.0

**Задача 2.6.1 — Упаковка: PPA, Debian repo, COPR**
- **What:** каналы дистрибуции: PPA (Ubuntu LTS, Mint), репозиторий Debian (trixie/bookworm), COPR (Fedora); подпись всех репозиториев.
- **Where:** `packaging/{debian,fedora,ppa}/`; инфраструктура репо.
- **How:** по спеке — дистро-репозитории после стабилизации T2-гарантий; AUR остаётся каналом community-провайдеров; NixOS и прочие — T3-путь (сборка есть, capabilities() честен, гарантий нет).
- **AC:**
  - [ ] Установка штатным менеджером на каждой T2-платформе → демон работает, TTFM < 5 мин (автотест онбординга на VM)
  - [ ] Подпись репозиториев проверена свежей машиной (без доверия к ключу установка честно предупреждает)
  - [ ] Обновление с MVP-версии на v1.0 без ручных шагов (Debian-машина с Arch-прошлого релиза не тестируется — миграция между семействами не обещается)

**Задача 2.6.2 — QA-матрица T2 и гейты tier**
- **What:** CI-контейнеры/VM по эталонным версиям T2 (smoke) + полный contract suite на каждой семье; гейт «tier ≠ capabilities» — расхождения фиксируются в отчёте релиза (§7.5).
- **Where:** CI; `docs/release-checklist.md`.
- **How:** T2 = полное чтение + безопасная запись, smoke на эталонных версиях (§6); Manjaro — полный набор T1 со своей матрицей; T3 bullseye — только сборка.
- **AC:**
  - [ ] Матрица 9 Linux-платформ зелёная (по своему уровню: T1 полный, T2 smoke, T3 build; состав — 8 строк матрицы спецификации плюс Ubuntu 26.04 LTS, решение Р-6)
  - [ ] Отчёт релиза содержит таблицу расхождений tier↔capabilities, если они есть
  - [ ] Демо №4 («С нуля»: пустая VM → машина под задачу по KB платформы) проходит на Arch и Ubuntu в CI

**Задача 2.6.3 — Релиз v1.0**
- **What:** tagged-релиз + обновление позиционирования (сегменты Ubuntu/Mint/Fedora/Debian из питча), Show HN-фоллоу-ап, статья Phoronix/Hackaday.
- **Where:** GitHub Releases; каналы питча.
- **AC:**
  - [ ] Заметки релиза: матрица 8 платформ, честные границы (apt без undo, prev_boot после opt-in, сеть read-only)
  - [ ] Метрики: TTFM по каждой платформе, north star в динамике с MVP
  - [ ] DoD MS2: демо 1–4 зелёные на эталонных платформах; наблюдение релиза (не гейт): в diff-логе `omnibus-core` с MVP только аддитивные расширения — регистрация `net_status` поверх трейта Ф2.2.3 и новые capability-константы

---

## 3. MS2.5 — v1.5: комплементарные подсистемы Linux

**Цель:** замкнуть агентный цикл на Linux: агент не только читает и чинит, но и запускает приложения, видит живое состояние системы и знает полный инвентарь пакетов. Решения ревизии 2026-11-07: **D1** — подсистема запуска приложений (трейт `AppLauncher`, Ф2.7); **D2** — подсистема процессов и телеметрии (трейт `ProcessTelemetry`, Ф2.8; чтение `/proc`, sysfs, UPower; сигнал — за грантом; PID 1 и юниты systemd исключены); **D3** — read-only инвентарь flatpak/snap в подсистеме packages без нового трейта (Ф2.9); **D4–D8** — расширение un-плана (§5.4); **D9** — лестница заявлений в питче («Честные границы»). Ставка прежняя (ADR-011): **ядро не меняется** — оба новых трейта объявляются just-in-time вместе со своими первыми реализациями, реестр §4 архитектуры остаётся закрытым списком задач этого роадмапа; v1.5 — только 8 Linux-платформ матрицы §6 (macOS — вне v1.5, подсистемы появятся там своими адаптерами по мере спроса).

### Ф2.7. adapter-launch: запуск приложений (AppLauncher)

**Задача 2.7.1 — Трейт `AppLauncher` + contract suite**
- **What:** объявить подсистему запуска приложений: трейт `AppLauncher` с операциями `list`, `launch(AppSpec{app_id, args})`, `open(uri|path)`; capability-константы `app.read`, `app.launch`; DTO-дисциплина общая (именованные структуры, newtype-обёртки, нативные типы не покидают адаптер). Трейт объявляется **just-in-time** — здесь же, вместе с первой реализацией (2.7.2); заблаговременных заглушек нет.
- **Where:** `omnibus-contracts/src/spi/app-launcher.rs`; suite — `omnibus-contracts/src/suite/` (семейство `app_launcher_suite`, связывается из `tests/contract_suite.rs` адаптера).
- **How:** R1 — контракт раньше кода: инструменты `app.list`/`app.launch`/`app.open` регистрируются в ядре только после объявления констант и трейта; каждый метод возвращает контрактную ошибку, `NativeFailure{code}` сохраняет payload исходного сообщения.
- **AC:**
  - [x] Трейт и дескриптор capabilities (`app.read`, `app.launch`) объявлены; semver контракта проставлен и независим от версии кода
  - [x] Suite `app_launcher_suite` компилируется и гоняется против фейковой реализации; инструкция привязки — в README контрактов (как в 1.1.4)
  - [x] Ни одного tuple-параметра и нативного типа в сигнатурах (линты задачи 1.1.3 зелёные)

**Задача 2.7.2 — adapter-launch: `app.list`, `app.launch` (systemd-run --user), `app.open` (xdg-open)**
- **What:** реализация `AppLauncher` на Linux: `app.list` — инвентарь приложений из `.desktop`-файлов (`/usr/share/applications` + `$XDG_DATA_HOME`/`~/.local/share/applications`; `Exec`/`Name`/`Icon`/`NoDisplay`); `app.launch` — запуск приложения как transient user-юнита через `systemd-run --user` (процесс попадает в cgroup пользователя и в journald — наблюдаем `ps.list` и `journal_query`); `app.open` — открытие URI/файла ассоциированным приложением через `xdg-open`. `app.launch`/`app.open` — write-действия [+scope] за write-скоупом `app.launch`.
- **Where:** `omnibus-adapter-launch/src/`.
- **How:** P3 — запись только типизированными методами и только по гранту; свободных shell-команд нет: аргументы передаются списком в DTO, без `sh -c` (метасимволы не интерпретируются); пол systemd ≥ 252 наследуется от матрицы (systemd-run есть на всех целевых Linux-платформах); отсутствие `xdg-open` на стенде → `app.open` деградирует по лестнице §7.3 (инструмент скрыт, `capabilities()` объясняет причину).
- **AC:**
  - [ ] Contract suite `AppLauncher` зелёный: `app.list` отдаёт приложения эталонного стенда (GNOME и KDE); `app.launch` запускает тестовое приложение, его юнит и вывод видны в journald, процесс виден `ps.list` (замкнутый цикл: запуск → наблюдение)
  - [ ] `app.open` открывает файл и URI дефолтным приложением; на стенде без xdg-open инструмент скрыт без ошибок в журнале, `capabilities()` объясняет причину (R3)
  - [x] `app.launch`/`app.open` без write-скоупа → `ScopeDenied` + аудит-событие (R5); с грантом — событие с полной схемой `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}`
  - [x] Аргументы приложения не проходят через shell (ревью + тест: спецсимволы `;`, `|` и обратный апостроф в аргументах не интерпретируются)

### Ф2.8. adapter-proc: процессы и телеметрия (ProcessTelemetry)

**Задача 2.8.1 — Трейт `ProcessTelemetry` + contract suite**
- **What:** объявить подсистему процессов и телеметрии: трейт `ProcessTelemetry` с операциями `list`, `info(pid)`, `sys_stats`, `power_status`, `signal(pid, sig)`; capability-константы `ps.read`, `ps.signal`, `sys.read`, `power.read`; сигналы — закрытый enum в контракте (не число). Just-in-time, вместе с реализацией 2.8.2.
- **Where:** `omnibus-contracts/src/spi/process-telemetry.rs`; suite — `process_telemetry_suite`.
- **How:** R1; юниты — newtype-обёртки по дисциплине DTO §4 (`Percent`, `Bytes`, `Milliseconds`, `Seconds`); каждый метод возвращает контрактную ошибку.
- **AC:**
  - [x] Трейт и дескриптор capabilities объявлены; semver контракта проставлен
  - [x] Suite гоняется против фейковой реализации, включая негативные кейсы: несуществующий PID → контрактная ошибка, не паника

**Задача 2.8.2 — adapter-proc: чтение `/proc`, sysfs, UPower**
- **What:** реализация read-части: `ps.list`/`ps.info` — процессы из `/proc` (pid, имя, состояние, CPU, RSS, cgroup; лимиты/фильтры в DTO — как у `journal_query`); `sys.stats` — загрузка CPU/RAM, load average, память/swap (`/proc`, `/sys`); `power.status` — батарея и питание через UPower D-Bus «когда есть». **PID 1 и процессы systemd-юнитов исключены из выдачи** — сервисы адресуются только через подсистему сервисов (`svc_list`/`svc_status`): никакого двойного адресного пространства.
- **Where:** `omnibus-adapter-proc/src/`.
- **How:** чтение — всегда (P3); UPower — авто-детект probe'ом: десктоп без батареи/UPower → `power.status` скрыт, `capabilities()` объясняет (R3/R7); чтение `/proc` чужих пользователей не расширяет права: отдаём то, что видно под uid демона, без эскалаций.
- **AC:**
  - [ ] Contract suite `ProcessTelemetry` (read-часть) зелёный: `ps.list` на системе с 300+ процессами в бюджете времени; `ps.info` отдаёт целостный снапшот по PID
  - [x] PID 1 и процессы systemd-юнитов отсутствуют в `ps.list`/`ps.info` (тест); юниты адресуются только через `svc_*`
  - [x] Стенд без UPower/батареи → `power.status` скрыт без ошибок в журнале (честность R7: не обещаем того, чего нет)

**Задача 2.8.3 — `ps.signal` за грантом + границы записи**
- **What:** write-часть: `ps.signal` [+scope] — отправка сигнала процессу (enum: TERM, INT, HUP, USR1, USR2, KILL) за write-скоупом `ps.signal`; адресация — только по `pid` из `ps.list` (фактом, не именем-догадкой); PID 1 и процессы systemd-юнитов исключены и здесь — сигнал по ним → `Unsupported` с подсказкой «сервисы — через `svc_stop`» (никакого «перезапуска системы под видом процесса»).
- **Where:** `omnibus-adapter-proc/src/write.rs`; гейтинг write-скоупа — ядро.
- **How:** P3/R5 — сигнал типизирован, аудит обязателен; `omnibusd grant ps.signal` — тот же единый механизм грантов (записи в TOML-манифестах, provenance `config|cli|doctor`).
- **AC:**
  - [x] `ps.signal` без write-скоупа → `ScopeDenied` + аудит-событие; с грантом сигнал доходит до тестового процесса (интеграционный тест)
  - [x] Сигнал по PID 1 / процессу systemd-юнита → `Unsupported` с подсказкой svc-инструментов (тест)
  - [x] Произвольный номер сигнала не принимается — только enum контракта (тест на негативном примере)

### Ф2.9. Инвентарь flatpak/snap, QA и релиз v1.5

**Задача 2.9.1 — adapter-flatpak: read-only инвентарь flatpak/snap**
- **What:** read-only инвентарь Flatpak и Snap в подсистеме packages: **новый трейт не вводится** — адаптер реализует read-часть `PackageManager` (`list`, `info`); `pkg_list`/`pkg_info` агрегируют нативный менеджер и инвентарь flatpak/snap (поле «источник» в DTO: `native`/`flatpak`/`snap`); install/remove/undo для flatpak/snap не объявляются — честное ограничение в схеме и в `capabilities()`.
- **Where:** `omnibus-adapter-flatpak/src/` (flatpak и snap — один крейт-инвентарь: обе команды вызываются через свои CLI-бинарники, хэндлы не покидают адаптер).
- **How:** probe активирует нативный менеджер и инвентарь-адаптеры вместе — единственное документированное исключение правила «один активный адаптер на подсистему» (§9 архитектуры); flatpak/snap не установлены → источник просто отсутствует в выдаче, никаких обещаний (R7); contract suite `PackageManager` (read-часть) — без изменений (R2: контракт тот же).
- **AC:**
  - [x] `pkg_list` на стенде с flatpak- и snap-приложениями показывает их с корректным источником; `pkg_info` отдаёт метаданные; без flatpak/snap выдача не меняется и не деградирует
  - [x] Write-операции для flatpak/snap отсутствуют: адаптер не объявляет `install`/`remove`/`undo` (ревью + контракт реализации)
  - [ ] Contract suite `PackageManager` (read-часть) зелёный против агрегата alpm+flatpak на Arch-стенде и apt+flatpak на Ubuntu-стенде

**Задача 2.9.2 — QA-матрица v1.5 и релиз**
- **What:** smoke новых инструментов на эталонных платформах (Arch, Manjaro, Ubuntu LTS, Fedora — минимум), те же каналы упаковки (минорный релиз в существующие репозитории), релиз v1.5 с лестницей заявлений D9.
- **Where:** CI; `docs/release-checklist.md`; GitHub Releases.
- **How:** v1.5 — аддитивное расширение: в diff-логе `omnibus-core` с v1.0 только регистрации новых инструментов поверх трейтов Ф2.7/Ф2.8 и новые capability-константы (наблюдение релиза, не гейт — как в 2.6.3); контракты — минорная версия; заметки релиза повторяют лестницу заявлений питча: v1.0 — полный контракт на целевых дистрибутивах, v1.5 — замкнутый цикл + живое состояние + полный инвентарь; сильная форма «AI-native OS» не заявляется.
- **AC:**
  - [ ] Сценарий «замкнутый цикл» зелёный в CI: `app.launch` по гранту → `ps.list` видит процесс → `journal_query` читает вывод → `ps.signal` по гранту завершает
  - [x] Заметки релиза: новые capabilities (`app.read`, `app.launch`, `ps.read`, `ps.signal`, `sys.read`, `power.read`), честные границы (flatpak/snap — read-only, `power.status` «когда есть», запуск и сигнал — за грантом), лестница заявлений D9
  - [ ] Наблюдение релиза: ядро не менялось — только аддитивные регистрации; TTFM < 5 мин сохраняется на эталонных платформах (автотест онбординга)

---

## 4. MS3 — v2.0: macOS T2

**Цель:** тот же контракт на macOS ≥ 14 (рекомендуемый 15+): адаптеры ax / unifiedlog / launchd / brew(+mas) / screencapture / fs-tcc / kb-guides; TCC-визард в doctor; Homebrew formula с подписью и нотаризацией; AppIntentsBridge как опциональный бэкенд. Проверка главной архитектурной ставки: **ни одного изменения существующих контрактов и инструментов — только аддитивные расширения**: capability-константы `intents.*` и новые инструменты §5 (`intent_list`/`intent_run`) поверх трейта `AppIntentsBridge`, объявленного в Ф3.6.2 вместе с реализацией, — минорная версия контракта (`files.scope.tcc` объявлена ещё в контрактах MS1 — задача Ф1.2.3 — и в diff MS3 не входит).

### Ф3.1. macOS-гейт предположений

**Задача 3.1.1 — Верификация ⚠M0-допущений (macOS-часть)**
- **What:** качество Rust-байндингов к AX (objc2) — прототип tree/element/set_value/invoke на 3 приложениях (нативное, Electron, браузер); поведение TCC в headless/SSH-сессиях; таргетинг: macOS 14 vs 15 (ServiceManagement 13+, ежемесячный re-grant Screen Recording в 15+); ограничения retention unified log (окно хранения прошбутовых записей — определяет фактическое наличие `journal.prev_boot` на macOS и его поведение при вытеснении старых записей); фактическое значение `pkg.undo` для brew (ожидание: none — brew не даёт транзакционного отката; подтвердить и задокументировать).
- **Where:** `research/macos-m0/` (прототип), результаты — `docs/verification/macos-findings.md`.
- **How:** минимальный Xcode-free прототип на objc2; тесты на физической машине и на CI-раннере; вывод по таргетингу — в §6 архитектуры (следующая ревизия).
- **AC:**
  - [ ] Прототип читает дерево и делает typed write на 3 типах приложений; известные проблемы байндингов задокументированы с оценкой трудоёмкости
  - [ ] Headless/SSH-поведение TCC зафиксировано (что работает без GUI-грантов, что нет) — влияет на CI-стратегию Ф3.7
  - [ ] Таргетинг выбран и обоснован (min version + recommended); pkg.undo(brew) зафиксирован в константе capabilities
  - [ ] Ограничения retention unified log измерены и задокументированы: что происходит с `journal.prev_boot`, когда старые записи вытесняются за окно хранения
  - [ ] Если прототип выявил блокер контракта (например, невозможность typed write) — эскалация в архитектуру до старта Ф3.2

### Ф3.2. adapter-ax (desktop)

**Задача 3.2.1 — DesktopA11y на AXUIElement: полный путь**
- **What:** `tree(app)`, `element(id)`, `read_attr`, `set_value`, `invoke` через NSAccessibility/AXUIElement; system-wide и per-pid таргетинг.
- **Where:** `omnibus-adapter-ax/src/`.
- **How:** AX API зрелее AT-SPI2 — контракт тот же, suite без изменений (R2); trust-checks и транзакционность действий — по известным практикам AX-автоматизации; Electron-приложения — a11y-флаг (doctor, Ф3.6).
- **AC:**
  - [ ] Contract suite `DesktopA11y` зелёный на macOS-раннере: Finder, TextEdit (нативные), Electron-приложение, браузер
  - [ ] Демо №2 («Формы») проходит на macOS: fs-tcc → ax set_value/invoke
  - [ ] Отзыв гранта Accessibility в TCC → адаптер деградирует по лестнице (opt-in-required), doctor-визард ведёт обратно

### Ф3.3. adapter-unifiedlog (журнал + аудит)

**Задача 3.3.1 — Journal на unified log + AuditSink**
- **What:** `query(Filter{...})`, `tail` через `log show --predicate` (или private API-альтернативу — ADR-004); AuditSink → unified log (subsystem/category omnibus).
- **Where:** `omnibus-adapter-unifiedlog/src/`.
- **How:** схема события аудита принадлежит ядру — приёмник сменный (P5); `journal.persistent` на macOS всегда true (unified log персистентен в рамках retention) — проверить retention-ограничения в Ф3.1 и задокументировать.
- **AC:**
  - [ ] Contract suite `Journal` зелёный; фильтры severity/window маппятся на predicate-синтаксис; unit → typed `Unsupported` с подсказкой (severity/window/process) (тесты на синтетических событиях)
  - [ ] Аудит-событие Omnibus читается обратно из unified log тем же адаптером (круговорот: write → query)
  - [ ] Бюджет производительности `log show` на большом журнале зафиксирован в тесте (page/лимиты)

### Ф3.4. adapter-launchd и adapter-brew(+mas)

**Задача 3.4.1 — ServiceManager на launchd**
- **What:** `list`, `status`, `start/stop/restart`, `enable/disable` через launchctl; LaunchAgent-контекст (только агенты видят GUI); ServiceManagement 13+ для регистрации.
- **Where:** `omnibus-adapter-launchd/src/`.
- **How:** domain-различие GUI/системных сервисов — в DTO (`Scope::User`/`System`); не поднимаем системные daemon'ы из сессии (политика ядра).
- **AC:**
  - [ ] Contract suite `ServiceManager` зелёный: рестарт пользовательского LaunchAgent доходит до активного состояния
  - [ ] Системный дом без root → честная контрактная ошибка, а не тихий отказ
  - [ ] GUI-видимость: сервис, запущенный как агент, отображается в сессии (smoke с тестовым LaunchAgent)

**Задача 3.4.2 — PackageManager на brew(+mas)**
- **What:** `list`, `info`, `search`, `install`, `remove`, `history` через brew; mas — опциональный бэкенд App Store; `pkg.undo` по факту Ф3.1 (ожидаемо none — честная деградация по §7.3.1).
- **Where:** `omnibus-adapter-brew/src/`.
- **How:** cask+formula в одном контракте (тип пакета — поле DTO); mas за capability-флагом (установлен/не установлен).
- **AC:**
  - [ ] Contract suite зелёный (formula и cask); демо «С нуля» на macOS ставит инструментарий через brew по статьям KB
  - [ ] Схема pkg_install помечает «undo недоступен», если `pkg.undo=none` подтверждён
  - [ ] mas отсутствует → App Store-пакеты скрыты из search, остальные работают

### Ф3.5. Порталы, файлы: screencapture / fs-tcc

**Задача 3.5.1 — ScreenCapture + Notify + clipboard**
- **What:** `screenshot` через ScreenCaptureKit (с учётом ежемесячного re-grant права в 15+); `notify` через UNUserNotificationCenter; `clip_read`/`clip_write` через NSPasteboard.
- **Where:** `omnibus-adapter-screencapture/src/`.
- **How:** capability `portal.screenshot` = opt-in-required до гранта Screen Recording; re-grant-напоминания — doctor (Ф3.6).
- **AC:**
  - [ ] Round-trip: скриншот, уведомление, clipboard — на macOS 14 и 15 (CI-раннер + физическая машина)
  - [ ] Истёкший/отозванный грант → деградация по лестнице, doctor объясняет про ежемесячный re-grant
  - [ ] Без гранта screenshot скрыт; clipboard/notify не требуют Screen Recording и работают

**Задача 3.5.2 — FileScope: fs-tcc**
- **What:** `fs_list`/`fs_read`/`fs_write` в TCC-грантированных папках (`~/Documents` и др.); за пределами — Full Disk Access; capability `files.scope.tcc`.
- **Where:** `omnibus-adapter-fs-tcc/src/`.
- **How:** скоупы macOS — это официальные права ОС (питч: «права, а не конвенция»); отсутствие гранта → opt-in-required, doctor-визард.
- **AC:**
  - [ ] В грантированной папке — полный read/write; вне (например, `~/Library`) без FDA → честный отказ с подсказкой doctor
  - [ ] Скоуп-модель ядра ограничивает поверх TCC (пользовательский TOML-грант уже; TCC — необходимо, но не достаточно — тест: TCC есть, скоупа нет → `ScopeDenied`)

### Ф3.6. Знания и опциональный бэкенд: kb-guides + AppIntentsBridge

**Задача 3.6.1 — KnowledgeBase на kb-guides**
- **What:** агрегатор гайдов (Apple Support, SS64, сообщество) через контракт `KnowledgeBase`; без плейбуков — применимость гайда оценивает агент (P7).
- **Where:** `omnibus-adapter-kb-guides/src/`.
- **How:** честная оговорка спеки — канонического ArchWiki-эквивалента нет, качество macOS-контента — слабейшее звено macOS-контракта (риск §6 спеки); кэш статей как в Ф1.5.6.
- **AC:**
  - [ ] `wiki_search` находит известное fix-руководство; демо №4 проходит на чистой macOS-VM по статьям KB
  - [ ] Оговорка качества (нет канонической вики, модерация сообщества) задокументирована; `capabilities()` отражает провайдера

**Задача 3.6.2 — AppIntentsBridge (опциональный бэкенд)**
- **What:** инструменты ядра `intent_list` / `intent_run` [+scope] поверх операций трейта `list_actions`, `run` — через Shortcuts/App Intents, только cooperating-приложения; бэкенд, не замена a11y-слоя.
- **Where:** `omnibus-adapter-appintents/src/`.
- **How:** трейт `AppIntentsBridge` объявляется здесь же, в `omnibus-contracts`, вместе с первой реализацией (just-in-time, аддитивно) — ядро не меняется; инструменты поверх bridge появляются в tool list только при наличии actions (probe-отчёт).
- **AC:**
  - [ ] На машине с готовыми Shortcut'ами: list_actions видит их, run выполняет тестовый Shortcut; аудит-событие записано
  - [ ] На чистой машине bridge отсутствует в tool list без ошибок
  - [ ] Дифференциация в документации: когда агенту использовать ax, когда app-intents (док в `docs/`)

### Ф3.7. doctor-macOS, дистрибуция, CI, релиз v2.0

**Задача 3.7.1 — doctor: TCC-визард**
- **What:** ведёт через выдачу 4 грантов (Accessibility, Screen Recording, Automation, Full Disk Access); ежемесячный re-grant Screen Recording (15+) — напоминания по состоянию гранта; headless/SSH-ограничения из Ф3.1 задокументированы.
- **Where:** `omnibus-core/src/doctor.rs` (оркестрация — ядро), macOS-специфика в адаптерах.
- **How:** §7.4 — TCC выдаёт только человек через GUI или MDM/PPPC; doctor не «выдаёт», а ведёт (открывает нужную панель, проверяет состояние; изменения видны в новой сессии).
- **AC:**
  - [ ] Свежая macOS-VM: TTFM < 5 мин от `brew install omnibus` до первой задачи (визард уложился)
  - [ ] Отзыв любого гранта между сессиями → при старте сессии doctor показывает точный список, что просрочено
  - [ ] Doctor не пытается программно выдать TCC (ревью по никогда-списку; тест: без GUI-взаимодействия грант не «появляется»)

**Задача 3.7.2 — Дистрибуция: Homebrew formula + подпись + нотаризация + LaunchAgent**
- **What:** formula в Homebrew; Developer ID-подпись и нотаризация бинарника; mas-интеграция опционально; без регистрации LaunchAgent — демон стартует как child MCP-клиента (stdio-топология §3.1).
- **Where:** `packaging/homebrew/`; CI-джобы подписи/нотаризации.
- **How:** install-time детекция §7.2 — macos-набор адаптеров; без подписи Gatekeeper честно предупреждает (задокументировать как T3-путь сборки из исходников).
- **AC:**
  - [ ] `brew install omnibus` на чистой macOS → нотаризованный бинарник; демон стартует как child MCP-клиента в сессии пользователя
  - [ ] Нотаризация в CI воспроизводима (secrets не в репо; джоба описана)
  - [ ] Обновление формулы с v2.0-rc на v2.0 — без ручных шагов

**Задача 3.7.3 — CI macOS + релиз v2.0**
- **What:** macOS-раннеры: contract suite против всех macOS-реализаций, TCC-сценарии (по ограничениям Ф3.1), smoke демо 1–4; релиз с матрицей 9 платформ.
- **Where:** CI; GitHub Releases.
- **How:** если headless-TCC ограничивает CI — полный TCC-набор гоняется на ферме физических машин (документированный процесс), раннеры — subset; позиционирование macOS-сегмента питча.
- **AC:**
  - [ ] Демо №1–4 зелёные на macOS-эталоне (демо №3 — в честной деградации «undo нет», если pkg.undo(brew)=none подтверждён)
  - [ ] `omnibus-core`: наблюдение релиза — в diff-логе с MVP только аддитивные расширения (capability-константы, регистрация `intent_list`/`intent_run` поверх трейта Ф3.6.2); контракты — минорная версия максимум
  - [ ] Заметки релиза: 9 платформ, tier-таблица, TCC-инструкция; DoD MS3 выполнен полностью

---

## 5. Трассировка и риски

### 5.1. Соответствие roadmap'у спеки (M0–M3)

| Спека | Этот роадмап |
|---|---|
| M0 (contracts+probe, каркас, atspi/journald/alpm read, скоупы, PKGBUILD, Xvfb-smoke) | Ф1.1–Ф1.5, Ф1.8.1 |
| M1 (write-пути X11/Wayland, systemd1 write, аудит, doctor, CI+contract suite на Arch) | Ф1.6–Ф1.8 |
| M2 (apt+dnf → T2, kb-guides, AUR-канал провайдеров) | Ф2.2–Ф2.6 |
| M3 (macOS T2: ax/unifiedlog/launchd/brew/fs-tcc, TCC-визард, Homebrew+нотаризация) | MS3 целиком |

### 5.2. Ключевые риски по майлстоунам (из §6 спеки)

| Майлстоун | Риск | Где закрывается |
|---|---|---|
| MS1 | Electron без a11y-флага | Ф1.7.1: doctor ведёт per-app enable |
| MS2 | Фрагментация дефолтов (volatile journald, NM не универсален) | Ф2.1.2, Ф2.2.3, Ф2.5.1: полы + opt-in + честные схемы |
| MS2 | QA-матрица 6 дистрибутивов × DE × X11/Wayland | Ф2.6.2: tier-модель (T2 = smoke на эталонах) |
| MS3 | TCC-трение (права только через GUI; re-grant Screen Recording) | Ф3.1, Ф3.7.1: визард + headless-исследование до старта работ |
| MS3 | objc2-байндинги слабее ожиданий | Ф3.1.1 AC-4: эскалация до Ф3.2 |
| Все | Гонка с вендорами (RHEL Lightspeed, Apple/Siri) | Не закрывается задачами: смягчается скоростью MS1→MS2 и открытым контрактом (MCP, ACP) |

### 5.3. Метрики успеха (по питчу)

- **TTFM < 5 минут** — на каждой платформе своего tier к концу её майлстоуна (замер — автотест онбординга: Ф1.8, Ф2.6, Ф3.7).
- **North star** — успешные агентные задачи в неделю: замеряется с релиза MS1 без телеметрии (опрос + трекинг issues).
- Retention D30, % задач без ручного вмешательства — пересматриваются на каждом релизе.

### 5.4. Что осознанно не планируется (un-план)

Сетевые write-действия; смена сетевого бэкенда; бэкпорты systemd; программная выдача TCC; consent-брокер с UI — никогда (гранты только вне контура агента: TOML-конфиг, `omnibusd grant`, `omnibusd doctor` — CLI, исполняет человек; in-band подтверждение от агента неотличимо от подделки); собственный формат плагинов (MCP — стандарт); серверные/headless-профили ОС; вынос `omnibus-contracts` в отдельный репозиторий — только при первом внешнем контрибьюторе адаптера (§8.8.3, решение не по календарю); модельные вызовы из omnibusd — никогда (инференс целиком в контуре агента юзера); local-ML в сервере — только теоретический остаток: embeddings для поиска (миллисекундные модели, не LLM), как опциональный адаптер, если когда-нибудь понадобятся; событийный канал a11y — вне v1 (ни push, ни буферизованный поллинг): watch-сценарии покрываются опросом `journal_query`/`svc_status`, наблюдение UI — опросом `desktop.tree` (пересмотр — при первом сценарии, которому опроса мало; push зависит от разнородной поддержки подписок в MCP-клиентах); эмуляция ввода (ydotool/uinput на Linux, CGEvent на macOS) — вне v1: typed-запись покрывает целевые сценарии, «последняя инстанция» вернётся opt-in-расширением только при доказанном кейсе; постоянный демон, admin-канал (сокет/D-Bus) и HTTP-транспорт — вне v1 (stdio-child покрывает целевые сценарии, координация инстансов — file-lock; пересмотр — при серверных/headless-профилях); управление звуком и медиа (MPRIS) — вне v1.5: повторяющихся сценариев, не покрываемых desktop-подсистемой (typed `invoke` на UI плеера), не набрано; пересмотр — при первом доказанном сценарии, которому MPRIS-семантики нужно больше, чем a11y-адресации (решение D4); управление окнами (фокус, раскладка, рабочие столы, тайлинг) — вне v1.5: на Linux нет единого протокола (X11/EWMH против разнородных Wayland-механизмов), на macOS — приватные API; пересмотр — при стандартизации протокола управления окнами в xdg-desktop-portal или первом сценарии, не покрываемом a11y-адресацией (D5); чтение уведомлений (история) — вне v1.5: у порталов notify write-only, read-интерфейса нет; пересмотр — при появлении read-API уведомлений в xdg-desktop-portal (до того история уведомлений не обещается; D6); write-действия питания и сессии (suspend/reboot/logout, смена сессии) — вне v1.5: терминальные состояния системы не делегируются из общего write-скоупа; чтение питания есть (`power.status`, Ф2.8); пересмотр — только как отдельный грант-тип с собственным аудитом при устойчивом спросе, не в общем списке write (D7); отдельная подсистема «настройки ОС» (dconf/gsettings-мост) не планируется — позиция settings-via-a11y: настройки меняются typed-записью через a11y-слой (`desktop.set_value` в UI настроек) или правкой конфигов в файловых скоупах; пересмотр — при доказанном повторяющемся сценарии, которому нужна структурированная семантика настроек; тогда — отдельная подсистема за своим трейтом (ADR-011), не спец-режим существующих инструментов (D8).
