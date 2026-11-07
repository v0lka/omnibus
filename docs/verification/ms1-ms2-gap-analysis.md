# Оценка полноты Linux-части роадмапа до MS2 включительно (gap-анализ)

**Дата аудита:** 2026-10-17.
**Объём:** все 40 атомарных задач разделов 1 и 2 `docs/development/omnibus-roadmap.md`
(1.1.1–1.8.3 из MS1, 2.1.1–2.6.3 из MS2).
**Метод:** настольный аудит только чтением. Каждая задача сверена с фактическим
состоянием репозитория: код и тесты (workspace, крейты, интеграционные тесты),
CI-определения (`.github/workflows/`, `ci/matrix/`, `ci/smoke/`), упаковка
(`packaging/`), документы верификации (`docs/verification/`), спеки (`specs/`),
состояние git. Тесты не запускались, прогоны CI не инспектировались (машинных
артефактов прогонов в репозитории нет).

**Правило присвоения статуса:**

- **выполнена** — все AC задачи подтверждаются чтением кода/тестов/артефактов
  репозитория;
- **частично** — кодовая/артефактная часть есть, но часть AC требует внешних
  сред (VM-матрицы, реальные раннеры, публикация каналов, релизные события)
  либо реализация отклоняется от буквы AC;
- **не начата** — deliverable задачи в репозитории отсутствует.

**Оговорка о «заявленных прогонах».** Два протокола приёмки —
`docs/verification/ms1-ms2-manjaro-acceptance.md` (2026-10-10) и
`docs/verification/ms2-acceptance.md` (2026-10-14/15) — ссылаются на выполненные
в тех сессиях прогоны (462 теста workspace, 150 упаковочных проверок, live-сессии
демона, coverage 77,63 %). Настольный аудит подтвердил существование в коде всех
выборочно проверенных тестов и инструментов (20+ имён тестов найдены ровно там,
где протоколы их цитируют), но сами прогоны чтением не воспроизводимы; ниже они
помечены словами «заявленный прогон». Выборочно проверенные имена тестов:
`element_token_of_one_instance_is_stale_in_another`,
`concurrent_writes_serialize_and_audit_records_acquisition_order`,
`exactly_one_full_schema_event_per_dispatch`, `params_hash_never_carries_raw_parameters`,
`in_band_confirmation_fields_cannot_unlock_a_write`, `default_install_is_read_only`,
`volatile_journal_doctor_reports_opt_in_with_both_human_levers`,
`applied_storage_mutation_is_one_audit_event`,
`declined_opt_in_leaves_the_status_quo_without_nagging`,
`prev_boot_query_refused_until_persistent_granted`,
`undo_none_marks_only_the_pkg_write_schemas`, `transaction_args_are_non_tty`,
`undo_refuses_before_any_native_work`, `contract_suite_passes_on_the_fixture_backend`
(apt, dnf), `report_declares_transactional_undo`,
`kb_suite_passes_against_the_offline_fixture`,
`candidates_select_the_family_backend_and_knowledge_source`,
`manjaro_report_carries_full_t1_candidates_and_facts`,
`debian_bookworm_is_on_the_floor_and_selects_apt_with_guides`,
`debian_bullseye_below_floor_reports_candidates_but_floor_unmet`,
`kb_candidate_follows_the_knowledge_column`.

---

## 0. Обновление аудита: репо-часть закрыта (ревизия 2026-10-17, вторая)

Все устранимые в репозитории пункты настоящего анализа закрыты по одобренному
набору решений (Р-1..Р-6 — исполнены, Р-5 — флор подтверждён без изменений,
Р-7/Р-8 — вынесены в runbook как чисто организационные). Исполнение:

- **Р-1 (исполнено):** `PkgUndoValue` — типизированный enum значений
  `pkg.undo` (`none` / `cache-downgrade` / `transactional`) в
  `omnibus-contracts/src/capabilities.rs`; адаптеры alpm/apt/dnf отчитываются
  значением через enum; packages-suite парсит факт через enum (вне-реестровое
  значение — провал suite); ядро (`degradation_note`) разбирает значение через
  enum. Wire-формы JSON не изменились.
- **Р-2 (исполнено):** unchecked-конструктор mint-ключа удалён — единственный
  конструктор `MintKey::try_from_entropy`, легитимный вызывающий — только
  композишн-рут; негативный compile-тест (compile_fail-доктест: нет
  публичного конструктора из сырой строки) и негативный рантайм-тест
  `forged_token_is_rejected`
  (`omnibus-core/tests/session_lifecycle.rs`): вымышленный токен в
  корректном wire-формате отвергается `ElementStale` до адаптера. Ограничение
  Rust (нельзя compile-time различить core/адаптер) честно задокументировано
  в `omnibus-contracts/src/tokens.rs` и README контрактов.
- **Р-3 (исполнено):** AC 1.4.1 и 1.8.2
  `docs/development/omnibus-roadmap.md` переписаны под отказ от оси DE в CI с
  зафиксированным критерием её ввода (первый шаг, ветвящийся по типу сессии);
  нормативная строка добавлена в `specs/architecture/platform-matrix.md`
  (раздел «CI Session Axes»); шапка `.github/workflows/ci.yml` не изменялась —
  критерий там уже был.
- **Р-4 (исполнено):** реализован wl-clipboard/xclip-бэкенд
  `clip_read`/`clip_write` (`omnibus-adapter-xdg-portals`): детекция по
  факту бинарников (/usr/bin, пара wl-copy+wl-paste либо xclip), budget-
  ограниченные подпроцессы (write — fire-and-forget из-за демона wl-copy),
  гейтинг `portal.clipboard` = present только при бэкенде, иначе факт
  отсутствует и инструменты скрыты; probe детектирует wl-clipboard/xclip в
  `backends_present` (1.4.1 What); тесты: round-trip на фикстурных
  бинарниках, живой round-trip contract suite, честные отказы, cap-лимит.
- **Р-5 (исполнено — «флор остаётся»):** флор AT-SPI2 2.50 сохранён без
  изменений во всех документах; подтверждение/коррекция по итогам 5-DE sweep
  — внешний шаг №8 (runbook B3).
- **Р-6 (исполнено):** Ubuntu 26.04 LTS добавлен строкой в
  `ci/matrix/t2.yml` (leg `ubuntu-26.04`, T2 smoke+contract, systemd 259) и в
  `docs/release-checklist.md` (§2 таблица легов, §4 таблица статусов); счет
  проверок валидатора 150 → 156; матрица — 9 Linux-платформ (счет в AC 2.6.2
  роадмапа синхронизирован).
- **Демо №4 (исполнено):** `ci/smoke/demo4-from-scratch.sh` написан и
  подключён к `ci/smoke/run-demos.sh` (леги: capabilities → doctor → KB →
  честный отказ pkg_install без гранта); тайм-аут KB-транспорта добавлен в
  honest-skip легов wiki_search демо 1 и 4 (окружение, не capability).
- **Runbook (подготовлен):** `docs/verification/external-steps-runbook.md` —
  ручная процедура блоков A–D раздела 5 и решений Р-7/Р-8.

### Таблица аудита: 23 ранее незакрытые задачи

Для каждой: закрыта ли репо-часть и что осталось. «Только раздел 5» = в
репозитории не осталось не выполненной работы; блокировка — исключительно
внешние шаги (раздел 5 / runbook).

| Задача | Было | Стало | Репо-часть | Осталось (раздел 5) |
|---|---|---|---|---|
| 1.1.1 | частично | только раздел 5 | выполнена (m0-findings, флор-константа, правки §6) | №8: 5-DE sweep, свежие инсталляции, bullseye-VM, gdbus (B1, B3) |
| 1.1.3 | частично | только раздел 5 | выполнена (линты, фикстуры, джобы) | №1–2: реальные прогоны CI, замер < 10 мин (A1, A2) |
| 1.2.2 | частично | **выполнена** | выполнена (Р-2: сужение API, compile_fail, `forged_token_is_rejected`) | — |
| 1.2.3 | частично | **выполнена** | выполнена (Р-1: enum `PkgUndoValue`, адаптеры, suite) | — |
| 1.4.1 | частично | только раздел 5 | выполнена (Р-3 AC; Р-4: детекция wl-clipboard/xclip в probe) | №1: реальный CI-прогон (A1) |
| 1.5.1 | частично | только раздел 5 | выполнена (suite привязан, бюджеты, тесты) | №9: Electron/GTK-леги, CI-прогон (B4) |
| 1.5.2 | частично | только раздел 5 | выполнена (suite, prev_boot-факты, тесты) | №7: двухстендовый volatile/persistent тест (B2) |
| 1.5.5 | частично | только раздел 5 | выполнена (Р-4: wl-clipboard/xclip-бэкенд, гейтинг, round-trip тесты) | №9: десктоп-матрица со скриншотом/notify (B4) |
| 1.6.1 | частично | только раздел 5 | выполнена (typed write, тесты) | №9: X11/ru-раскладка/HiDPI-леги (B4) |
| 1.7.1 | частично | только раздел 5 | выполнена (doctor MCP+CLI, opt-in, аудит) | №16: TTFM на чистой VM (C1) |
| 1.8.1 | частично | только раздел 5 | выполнена (PKGBUILD, проверки package-build) | №16: публикация AUR, установка, upgrade (C1) |
| 1.8.2 | частично | только раздел 5 | выполнена (Р-3 AC; workflow, демо 1–4) | №1, №3: реальные прогоны, branch protection (A1, A3) |
| 1.8.3 | частично | только раздел 5 | выполнена (черновик заметок, метрики, never-лист) | №17: тег, GitHub Release, анонсы (C2) |
| 2.1.1 | частично | только раздел 5 | выполнена (family-детекция, тесты кандидатов) | №6: VM-матрица 6 дистрибутивов (B1) |
| 2.1.2 | частично | только раздел 5 | выполнена (флоры bookworm/bullseye, тесты) | №6: VM-ноги (B1) |
| 2.3.1 | частично | только раздел 5 | выполнена (dnf undo, 29 тестов, suite) | №5: Fedora-прогон и откат (B7) |
| 2.4.1 | частично | только раздел 5 | выполнена (manjaro-факты, workflow) | №4, №10: VM-ноги, non-free smoke (A5, B5) |
| 2.5.1 | частично | только раздел 5 | выполнена (opt-in doctor, тесты) | №7: volatile-VM проход с перезагрузкой (B2) |
| 2.5.2 | частично | только раздел 5 | выполнена (kb-guides, 24 теста) | №15: реальные зеркала, пара машин (B10) |
| 2.6.1 | частично | только раздел 5 | выполнена (упаковка, PPA-поза, 156 проверок) | №19–21: публикация, без доверия к ключу, TTFM (D1–D3) |
| 2.6.2 | частично | только раздел 5 | выполнена (Р-6: ubuntu-26.04 leg; demo4 написан и подключён) | №4, №22: прогоны матрицы, demo4 на Arch/Ubuntu (A4, D4) |
| 2.6.3 | не начата | только раздел 5 | процедурные заготовки готовы (checklist §4–§5) | №23–24: тег, additive-аудит, релиз-отчёт (D5, D6) |

Итог ревизии: ни одна из 23 задач не имеет незакрытой репо-части; 2 закрыты
целиком (1.2.2, 1.2.3), 21 блокируются исключительно внешними шагами
раздела 5 (runbook: блоки A–D + Р-7/Р-8).

## 1. Сводка

| Раздел | Выполнена | Частично | Не начата |
|---|---|---|---|
| MS1 — задачи 1.1.1–1.8.3 (28) | 15 | 13 | 0 |
| MS2 — задачи 2.1.1–2.6.3 (12) | 2 | 9 | 1 |
| **Итого** | **17** | **22** | **1** |

Кодовая база Linux-части практически завершена: 16 членов workspace (контракты,
ядро, probe, `omnibusd`-композишн-рут, 11 адаптеров, лints), 13 ADR, 3 CI-воркфлоу,
декларативная матрица 8 Linux-платформ, упаковочные наборы для Arch/Debian/Fedora/PPA,
контракт suite, привязанный всеми 11 адаптерами. Незакрытый объём делится на три
категории: (а) внешние ноги — VM-матрицы, реальные прогоны CI, публикация каналов,
релизы (≈ 90 % остатка); (б) один не начатый кодовый артефакт — скрипт демо №4;
(в) горстка решений, где реализация отклоняется от буквы AC роадмапа и нужно либо
править документ, либо доделывать код (раздел 4).

---

## 2. MS1 — MVP: Arch Linux T1, полный контракт (раздел 1 роадмапа)

### Ф1.1. Гейт предположений и скелет проекта

**1.1.1 — Верификация ⚠M0-допущений (Linux-часть). Статус: частично.**
Доказательство: `docs/verification/m0-findings.md` — таблица 1 (персистентность
journald для 6 дистрибутивов, каждая строка с источником: artifact postinst/spec
Debian и Fedora, Launchpad для Ubuntu, release notes Mint; вывод — все 6
persistent по умолчанию, предположение «Ubuntu/Mint volatile» опровергнуто),
таблица 2 (systemd-версии: Ubuntu 24.04 = 255 — подтверждено, 26.04 = 259;
bookworm = 252 на полу; bullseye = 247 ниже пола), таблица 3 (AT-SPI2 —
дистро-стек, флор-константа `ATSPI_FLOOR = 2.50.0` в
`omnibus-contracts/src/floors.rs`). Правки §6 архитектуры внесены
(`docs/development/omnibus-architecture.md`, строки матрицы: bookworm
«at-spi2 2.46 < пола 2.50 — desktop.* скрыты»; per-machine модель персистентности
также в `specs/architecture/platform-matrix.md`).
Осталось (по AC): пер-DE замер минимальной версии AT-SPI2, на которой контрактный
набор проходит на всех пяти DE — «для каждой DE известно, проходит ли полный
контрактный набор» сегодня НЕ известно (KDE/XFCE/Cinnamon/MATE не измерялись);
подтверждение фактов таблицы 1 на свежих инсталляциях 6 дистрибутивов и после
первой перезагрузки; решение по bookworm (2.46 < 2.50): оставить флор с
документированной деградацией desktop.* на bookworm (текущий выбор, уже
задокументирован в матрице) или опустить флор по итогам sweep; негативный тест
bullseye на реальной VM; gdbus-инспект AT-SPI2-шины per DE per Wayland/X11.
Полный список — чек-лист «External runs required» в `docs/verification/m0-findings.md`.

**1.1.2 — Cargo workspace и карта крейтов. Статус: выполнена.**
Доказательство: корневой `Cargo.toml` — 16 членов (contracts, core, `omnibusd`,
probe, 11 адаптеров, lints); ADR-001 (`specs/decisions/001-dyn-trait-spi.md`,
указатель в `docs/architecture-decisions.md`); правило зависимостей enforced
линтом R4-EDGES (`omnibus-lints`), CI-джоба `dependency-rules` в
`.github/workflows/ci.yml` делает негативную проверку: в /tmp-клон инъектируется
запрещённое ребро adapter → core, линт обязан покраснеть (реализация AC «проверено
тестовым PR» как инлайн-шаг джобы, а не реальный PR — эквивалент по смыслу,
отмечено в разделе 6). Отклонение от буквы Where (добросовестное,
задокументировано): композишн-рут — отдельный тонкий крейт `omnibusd`, а не
бинарная цель внутри core, что жёстче изначальной формулировки и требуется линтом.

**1.1.3 — CI-каркас и connascence-линты. Статус: частично.**
Доказательство: `omnibus-lints` с 4 правилами (R4-COP, R4-NATIVE, R4-CON,
R4-EDGES); red/green-фикстуры (10 тестов, заявленный прогон E4/E5); джоба `gate`
в ci.yml гоняет fmt/clippy/test + оба линт-шага.
Осталось: AC «время прогона < 10 мин на кэшированном раннере» — замера нет
(в workflow `timeout-minutes: 20`, реальная длительность на раннере не
зафиксирована); реальное исполнение workflow на GitHub-раннере (в репо только
определения).

**1.1.4 — Contract test suite: harness. Статус: выполнена.**
Доказательство: `omnibus-contracts/src/suite/` — 13 семейных suite-функций
(a11y, journal, services, packages, screen_capture, notify, files, files_write,
kb, net, probe, audit_sink + шаблон события); инструкция по привязке —
`omnibus-contracts/README.md` (раздел «Binding the suite to an implementation»);
все 11 адаптеров привязаны suite (`src/lib.rs` либо `tests/contract_suite.rs`:
atspi, fs-xdg, nm, xdg-portals — в `tests/`, остальные — в `src/`); suites
исполняются в составе `cargo test --workspace` джобы `gate` (прогоны — заявленные).

### Ф1.2. omnibus-contracts: полная SPI

**1.2.1 — SPI контрактного слоя MS1: 10 трейтов (8 подсистем). Статус: выполнена.**
Доказательство: `omnibus-contracts/src/spi/` — 11 файлов трейтов: a11y
(DesktopA11y), journal (Journal), audit_sink (AuditSink), services
(ServiceManager), packages (PackageManager), portals (ScreenCapture + Notify),
files (FileScope), kb (KnowledgeBase), probe (PlatformProbe), net (NetworkStatus,
объявлен just-in-time вместе с Ф2.2.3, как предписано роадмапом; AppIntentsBridge
отсутствует до MS3 — корректно). Дескриптор capabilities у каждого трейта —
`report()` → AdapterReport с CapabilityFact (suite-проверка имён на константы
реестра). Ошибки — закрытая таксономия из 8 кодов (`omnibus-contracts/src/errors.rs`).
Semver: `CONTRACT_VERSION = "1.0.0"` (`omnibus-contracts/src/version.rs`) с тестом
`assert_ne!(CONTRACT_VERSION, CARGO_PKG_VERSION)` — независим от версий кода.

**1.2.2 — DTO-дисциплина и opaque-токены. Статус: частично.**
Доказательство: плоские DTO + newtype-обёртки, R4-COP/R4-NATIVE зелёные (заявленный
прогон); сериализация serde — единственное разделяемое поведение (schema round-trip
тесты); непрозрачность токенов подтверждена тестами
`omnibus-contracts/src/tokens.rs` (`tokens_are_opaque_prefixed_and_stable`,
`key_debug_withholds_material`) и `element_token_of_one_instance_is_stale_in_another`
(`omnibus-core/tests/session_lifecycle.rs`): чужой токен не резолвится (ElementStale).
Отклонение от буквы AC2 (сам протокол признал): AC требует «токены нельзя
сконструировать вне ядра (приватный конструктор, негативный compile-тест)» — в
коде `MintKey` публичен и имеет `from_entropy_unchecked`; теста
`forged_token_is_rejected` нет. Граница доверия реализована иначе: ядро не
раскрывает ключ mint, резолвятся только токены, отчеканенные самим инстансом.
Нужно решение (раздел 4, п. Р-2).

**1.2.3 — Реестр capabilities и error taxonomy. Статус: частично.**
Доказательство: `omnibus-contracts/src/capabilities.rs` — 18 констант, ровно
список §7.1 роадмапа (`ALL: [&str; 18]`, тест закрытости и уникальности);
формат имён и канонический алфавит; литералы вне реестра запрещены линтом
R4-CON; `CapabilityStatus` — enum Present/Degraded/OptInRequired; ошибки — 8
кодов, `anyhow`/`Box<dyn Error>` на границе нет (grep чистый).
Отклонение от буквы AC2: «pkg.undo типизирован enum'ом значений» — в коде
значения полиформенной capability передаются JSON-значениями-строками
(`"cache-downgrade"` в `omnibus-adapter-alpm/src/lib.rs`, `UNDO_VALUE`; «none» —
семантика отсутствия для apt-семейства), отдельного enum-типа значений в
контрактах нет. Нужно решение (раздел 4, п. Р-1).

### Ф1.3. omnibus-core: ядро omnibusd

**1.3.1 — MCP-фасад и реестр инструментов. Статус: выполнена.**
Доказательство: `omnibus-core/src/facade.rs`, `registry.rs`, `tools/`; rmcp-транспорт
stdio; схемы инструментов генерируются из DTO (`DtoSchema::schema()`, тест
строгости `every_schema_is_strict_object_json`); скрытие без capabilities — тесты
`default_install_is_read_only` (`omnibus-core/tests/scopes_grants.rs`) и
ladder-тест absent-capability → CapabilityRequired с подсказкой doctor.
Round-trip «из Claude Code/goose» выполнен через независимый stdio JSON-RPC
клиент `ci/smoke/mcp_client.py` (протокольная заявка E8; эквивалент по
протокольной поверхности, живой клиент из MCP-каталога не подключался).

**1.3.2 — Сессии. Статус: выполнена.**
Доказательство: `omnibus-core/src/session.rs`, `limits.rs`, `write_gate.rs`; все
четыре AC покрыты тестами `omnibus-core/tests/session_lifecycle.rs`:
`read_timeout_retries_once_then_times_out`, `write_timeout_is_never_retried`,
`concurrent_writes_serialize_and_audit_records_acquisition_order` (мьютекс +
flock через rustix), `element_token_of_one_instance_is_stale_in_another`,
`call_with_unregistered_slot_yields_adapter_unavailable` (автомат состояний).

**1.3.3 — Скоупы. Статус: выполнена.**
Доказательство: `omnibus-core/src/scopes.rs`, `grants.rs`; CLI `omnibusd grant`
(`omnibusd/src/grant.rs`, provenance `cli` в аудит-событии); шаблоны манифестов
`packaging/scopes.d/00-read-only.toml`; все пять AC с тестами:
`grantless_write_is_denied_and_audited`,
`in_band_confirmation_fields_cannot_unlock_a_write` + зеркальный
`consent_fields_are_ignored_when_the_grant_exists` (инвариант no-confirm-API),
provenance config|cli|doctor, `broken_manifest_is_recorded_and_healthy_scopes_keep_working`,
`default_install_is_read_only`.

**1.3.4 — Аудит-пайплайн. Статус: выполнена.**
Доказательство: `omnibus-core/src/audit.rs`; схема события из 9 полей; file-fallback
в ядре; AC с тестами `omnibus-core/tests/audit_pipeline.rs`:
`exactly_one_full_schema_event_per_dispatch`,
`sink_failure_degrades_to_fallback_without_failing_the_tool`,
`params_hash_never_carries_raw_parameters`.

**1.3.5 — Реестр capabilities() и гейтинг инструментов. Статус: выполнена.**
Доказательство: `omnibus-core/src/caps.rs`, `ladder.rs`; документ с
name/ver/status/value/adapter/note/remedy; лестница интеграционно
(`omnibus-core/tests/capabilities_ladder.rs`, `tests/ladder.rs`): degraded/absent/
below-floor/opt-in-tested; свежий probe на новый инстанс — структурно (detect()
на процесс) + `fresh_session_picks_up_the_new_grant_manifests`.

### Ф1.4. omnibus-probe и end-to-end деградация

**1.4.1 — PlatformProbe для Arch. Статус: частично.**
Доказательство: `omnibus-probe/src/detect.rs` (os-release, systemctl --version,
версия at-spi2-core из pacman -Q/dpkg/rpm), `manjaro.rs`, `runner.rs`; выбор
адаптеров — `omnibusd/src/wiring.rs`; liveness-строка at-spi2 гейтит desktop.*;
юнит-тесты выбора кандидатов; live-прогон на Manjaro GNOME/Wayland (заявленный).
Осталось: AC1 требует «GNOME и KDE, X11 и Wayland — 4 комбинации в CI» — CI
(`ci.yml` matrix-t1) гоняет только ось X11/Wayland; ось GNOME/KDE сознательно
не введена (обоснование R7 в шапке ci.yml: нет шага, ветвящегося по типу
сессии) — расхождение с буквой AC, нужно решение (раздел 4, п. Р-3); What
упоминает детекцию wl-clipboard/xclip — в probe её нет (grep пуст), фактически
бэкенды буфера обмена не детектируются вовсе (см. 1.5.5); реальный CI-прогон.

**1.4.2 — Version floor как механизм. Статус: выполнена.**
Доказательство: `omnibus-probe/src/floors.rs` импортирует SYSTEMD_FLOOR/ATSPI_FLOOR
из контрактов (тест значений констант); тест `below_floor_carries_the_exact_reason`
(«systemd 247.3 < 252.0»); ladder-тест FloorUnsupported; doctor-файлинг ниже пола
с ремедией; на Arch флор не срабатывает (rolling) — симулированный стаб по AC.

### Ф1.5. Адаптеры чтения (Arch)

**1.5.1 — adapter-atspi: read-путь. Статус: частично.**
Доказательство: `omnibus-adapter-atspi/src/` (4 файла, ~1800 строк); suite
DesktopA11y привязан (`tests/contract_suite.rs`); live-прогон по реальному
GNOME/Wayland: дерево gnome-shell 1851 узел (~1,7 с, заявленный прогон), бюджет
обхода 12 глубина/2000 узлов/12 с (юнит-тесты ledger).
Осталось: AC1 требует suite «на живом GTK- и Electron-приложении (в CI — Xvfb +
headless compositor)» — Electron-лег не подтверждён, CI-прогон матрицы не
исполнен; X11-лег.

**1.5.2 — adapter-journald: Journal. Статус: частично.**
Доказательство: `omnibus-adapter-journald/src/journal.rs` (JSON-фикстуры фильтров,
truncation); suite Journal привязан; prev_boot — по факту прошлых загрузок
(парсинг list-boots, юнит-тесты); live prev_boot на persistent-машине
(демо №1, заявленный прогон).
Осталось: вторая половина AC2 — «на volatile-стенде journal.prev_boot имеет
статус opt-in-required (тест на двух стендах)»: двухстендового теста нет;
поведение volatile покрыто юнит-логикой фактов и интеграционными тестами ядра
(`prev_boot_query_refused_until_persistent_granted`), но именно пары стендов —
внешний шаг.

**1.5.3 — adapter-systemd1: чтение. Статус: выполнена.**
Доказательство: `omnibus-adapter-systemd1/src/lib.rs`; suite read-части; live
svc_list/svc_status на 100+ юнитах с бюджетом (заявленный прогон); несуществующий
юнит → NativeFailure с сохранением текста D-Bus (тесты).

**1.5.4 — adapter-alpm: чтение. Статус: выполнена.**
Доказательство: `omnibus-adapter-alpm/src/`; suite read-части; live pkg_list на
1600+ пакетах, history из реального pacman.log (~1171 строка), version_available
из sync-db (заявленные прогоны).

**1.5.5 — adapter-xdg-portals: clipboard, screenshot, notify. Статус: частично.**
Доказательство: `omnibus-adapter-xdg-portals/src/`; suite + 10 wire/DTO тестов;
notify доставлен реально, screenshot с честью портального согласия (заявленный
прогон на GNOME/Wayland); отсутствие бэкенда → скрытие/деградация (тесты).
Ключевое отклонение: AC1 требует «round-trip clipboard» — его НЕТ и в текущем
виде быть не может: portal Clipboard требует RemoteDesktop-сессии, которую
адаптер сознательно не создаёт; `clip_read`/`clip_write` отвечают
`AdapterUnavailable` (lib.rs:447–481, честная деградация). Фолбэка wl-clipboard/
xclip (названного в How задачи) в коде нет ни в адаптере, ни в probe. Протокол
пометил этот AC PASS с оговоркой REQ-EXT — по факту требуемое поведение не
существует (см. раздел 6). Матрица «GNOME и KDE (X11 + Wayland)» не гонялась.
Нужно решение + возможно код (раздел 4, п. Р-4).

**1.5.6 — adapter-fs-xdg и adapter-kb-archwiki: чтение. Статус: выполнена.**
Доказательство: fs — suite + тесты границ (`read_outside_home_is_denied`), лимиты
2 МиБ/2000 записей, read-скоуп $HOME с dotfiles; kb-archwiki — live поиск
«suspend» + страница + stale-кэш с пометкой (юнит-тест unroutable endpoint),
manjaro-overlay-статьи (заявленные прогоны).

### Ф1.6. Адаптеры записи (Arch)

**1.6.1 — adapter-atspi: write-путь (typed). Статус: частично.**
Доказательство: типизированная запись без эмуляции клавиатуры: set_value через
свойство Value / `SetTextContents`, invoke через `DoAction` по имени действия
(`omnibus-adapter-atspi/src/lib.rs:1006–1162`); координат в API нет (код-ревью);
write без скоупа → ScopeDenied + аудит (`grantless_write_is_denied_and_audited`,
demo2). Осталось: AC1 требует зелёную write-часть suite «на X11 и Wayland, при
активной ru-раскладке включён отдельный тест» — live-прогон только GNOME/Wayland
без ru-лега; X11/ru-леги и HiDPI-стенд — внешние шаги.

**1.6.2 — adapter-systemd1: write. Статус: выполнена (в объёме suite-семантики).**
Доказательство: apply() с poll'ом JobRemoved и settle-бюджетом (TimeoutError на
зависание — сентинел-юнит suite); маппинг отказа polkit → NativeFailure с
человекочитаемой подсказкой (код 2, тест); отказ-события аудируются диспетчером.
Примечание: разрушительные рестарты живых юнитов сознательно не исполняются
тестами — семантика покрыта сентинел-дизайном suite; живой рестарт тестового
юнита до active (буква AC1) на реальной машине не зафиксирован протоколами
(относится к VM-матрице, раздел 5).

**1.6.3 — adapter-alpm: write + undo. Статус: выполнена.**
Доказательство: install/remove с честным NativeFailure на lock/privilege/disk;
undo = downgrade из кэша (`UNDO_VALUE = "cache-downgrade"`); цепочка
history → предыдущая версия → файл кэша → pkg_load покрыта юнит-тестами на
фикстурных раскладках лога/кэша (это и есть требуемый AC2 «интеграционный тест
с фейковым пакетом»); нет версии в кэше → NativeFailure
`undo-no-cache-version: previous version not in cache; see ALA/AUR archive`
(константа в lib.rs); pkg_undo скрыт при pkg.undo=none; живой downgrade —
REQ-EXT by design (никогда не исполняется против живой системы).

**1.6.4 — fs-xdg: write + сквозной аудит. Статус: выполнена.**
Доказательство: fs_write в грантированных скоупах; NEVER_WRITABLE префиксы
(/etc, /usr, /boot, /proc, /sys, /dev, /var + грант-стор + state-каталог) в
scope.rs с тестами (`write_outside_the_scope_is_denied`,
`outside_home_denied_without_a_grant`); сквозной аудит всех write-адаптеров
через единый пайплайн ядра (адаптеры без импортов аудита — P6).

### Ф1.7. doctor

**1.7.1 — doctor. Статус: частично.**
Доказательство: `omnibus-core/src/doctor.rs` — факт-движимый диагноз (персистентность
по probe-факту, а не по семейству); MCP-ветка read-only по построению (никаких
Command в MCP-пути, команды — строки в отчёте, executed_by="human"); CLI
`omnibusd doctor --apply <name>` с интерактивным YES и одним аудит-событием на
изменение (`omnibusd/src/doctor_cli.rs`); гранулярный opt-in; набор предложений
закрыт (journald-persistence + omnibusd grant) и вне never-листа (тесты
`omnibus-core/tests/doctor_optin.rs`, `tests/security.rs`).
Осталось: AC1 — «на свежей Arch-VM с GNOME/KDE: … TTFM < 5 минут (замер в тесте
онбординга)» — чистая VM не поднималась; замерен только автоматизируемый хвост
(инстанс → capabilities, 0,05 с, заявленный прогон). Полный TTFM — внешний шаг.

### Ф1.8. Упаковка, QA-гейты и релиз MVP

**1.8.1 — PKGBUILD и AUR. Статус: частично.**
Доказательство: `packaging/PKGBUILD` (сборка workspace-корня, установка бинарника,
шаблонов scopes.d, manjaro-overlay); CI-джоба package-build: `bash -n`,
`makepkg --printsrcinfo`, структурные проверки путей package()↔build().
Осталось: публикация в AUR — НЕ выполнена (вне репозитория по определению), AC
«pacman -S omnibus (из AUR) на чистой Arch-VM» и «обновление версии без ручных
шагов» на реальной машине невыполнимы до публикации; протокол честно помечает
это PASS-AS-PROXY.

**1.8.2 — CI: виртуальные сессии и полный гейт T1. Статус: частично.**
Доказательство: `ci.yml` — gate (specs+fmt+clippy+build+test+R4), dependency-rules
с негативной инъекцией, matrix-t1 (Arch-контейнеры, Xvfb/openbox для X11 и
weston headless для Wayland, `cargo test --workspace` + демо), coverage ≥ 75 %,
package-build; `ci/smoke/run-demos.sh` — демо №1–3 как автотесты (реальные
prev_boot-записи, desktop.tree по Wayland-сессии, честные ScopeDenied-леги;
заявленный прогон E8).
Осталось: реальное исполнение workflow (в репо только определения; branch
protection «красный main невозможен» — решение о хостинге); ось GNOME/KDE
отброшена с обоснованием R7 — расхождение с буквой AC «матрица CI: GNOME/KDE ×
X11/Wayland» (см. Р-3); демо-леги с грантами на живые записи сознательно за
границей CI.

**1.8.3 — Релиз MVP. Статус: частично.**
Доказательство: `docs/release-notes-0.1.0-draft.md` — черновик заметок с
capabilities-документом эталонной машины, известными ограничениями, never-листом
и заявлением об отсутствии телеметрии (grep сетевых отправок вне kb-адаптера
чистый — ревью подтверждаемо кодом).
Осталось: тега нет (`git tag` пуст — 0 тегов), GitHub Release не создан, AUR не
опубликован, Show HN / r/archlinux не публиковались, заявка на a11y-трек
GNOME/KDE не подана, north-star-трекинг не начат. Релиз как событие — не начат;
локальные артефакты (черновик, метрики-замер) — готовы частично.

---

## 3. MS2 — v1.0: все целевые дистрибутивы Linux (раздел 2 роадмапа)

### Ф2.1. Probe-обобщение: семейства и полы

**2.1.1 — Дистро-детекция и выбор семейства адаптеров. Статус: частично.**
Доказательство: `omnibus-probe/src/detect.rs` + `manjaro.rs`: os-release →
семейство (arch/manjaro/debian/ubuntu/mint/fedora), ровно один пакетный бэкенд
на семейство (alpm / apt / dnf), kb-источник по колонке знаний, nm — только при
живой шине (тесты `candidates_select_the_family_backend_and_knowledge_source`,
`manjaro_report_carries_full_t1_candidates_and_facts`); ManjaroFacts
(delayed_repos, mhwd, pamac) в отчёте; distro/floors публикуются в capabilities
(live: `distro {id: manjaro, family: manjaro, release: rolling}`, заявленный
прогон E10).
Осталось: AC1 — выбор на 6 целевых дистрибутивах «в VM-матрице CI» — VM-матрицы
нет (внешний шаг №1 раздела 5).

**2.1.2 — Полы на реальных дистрибутивах. Статус: частично.**
Доказательство: `omnibus-probe/src/floors.rs`: bookworm (252) — на полу, активен
(`debian_bookworm_is_on_the_floor_and_selects_apt_with_guides`); bullseye (247) —
ниже пола, точная причина «systemd 247.0 < 252.0»
(`debian_bullseye_below_floor_reports_candidates_but_floor_unmet`); композишн-рут
прячет pkg-семейство ниже пола даже при наличии apt
(`pkg_candidate_stays_empty_without_backend_floor_or_family`); document
объясняет только непройденные флоры (`document_explains_unmet_floors_only`);
volatile-журнал → opt-in-required с обоими рычагами
(`volatile_journal_doctor_reports_opt_in_with_both_human_levers`); prev_boot
гейтится грантом (`prev_boot_query_refused_until_persistent_granted`).
Осталось: реальные bullseye/bookworm/Ubuntu/Mint VM-ноги (в т.ч. автотест
«на полу» на живом bookworm и `journal.persistent=false` в живом документе на
Ubuntu/Mint) — внешний шаг №1.

### Ф2.2. adapter-apt

**2.2.1 — PackageManager на apt: чтение. Статус: частично.**
Доказательство: `omnibus-adapter-apt/src/` (~1800 строк): 24 теста, suite
 PackageManager привязан с `PackagesFixture { undo_expected: false }`; все
subprocess через один bounded-capture путь с бюджетом 9 с < 10 с бюджета
инструмента; тайм-бюджет отвечает честным `NF_TIMEOUT_INDETERMINATE`; t2.yml
гоняет ровно `cargo test --workspace` на trixie/bookworm (wiring подтверждён
валидатором, 150 проверок).
Осталось: реальные прогоны контейнеров trixie/bookworm и VM-ноги Ubuntu LTS и
Mint; замер pkg_list/pkg_info на системе с ~2000 пакетов — внешний шаг №8.

**2.2.2 — apt: write и честное «undo нет». Статус: выполнена.**
Доказательство: схемы pkg_install/pkg_remove при pkg.undo=none несут явную
пометку `UNDO_UNAVAILABLE_NOTE`, pkg_list чист, pkg_undo скрыт
(`undo_none_marks_only_the_pkg_write_schemas`, `omnibus-core/tests/capabilities_ladder.rs`);
undo отказывает до любой нативной работы
(`undo_refuses_before_any_native_work`); non-tty: флаги `-y`,
`-o=Dpkg::Use-Pty=0`, `DEBIAN_FRONTEND=noninteractive` закреплены тестами
(`transaction_args_are_non_tty`, fixture apt-get выходит 95 без флагов).
Живое подтверждение на apt-машине входит в VM-матрицу (внешний шаг).

**2.2.3 — NM read-only «когда есть». Статус: выполнена.**
Доказательство: `omnibus-adapter-nm/src/` — grep на write-методы даёт 0
совпадений; трейт NetworkStatus (`omnibus-contracts/src/spi/net.rs`) объявляет
только read `status()`; candidate nm — только при живой шине, connect() без NM →
AdapterUnavailable, скрытие лестницей без ошибок (тесты); live net_status в
списке 17/27 инструментов на NM-машине (заявленный прогон); прямой вызов без
NM → CapabilityRequired (ladder-тест). networkd/no-NM стенды — внешний шаг №10.

### Ф2.3. adapter-dnf

**2.3.1 — PackageManager на dnf + transactional undo. Статус: частично.**
Доказательство: `omnibus-adapter-dnf/src/` (~2100 строк): 29 тестов, suite с
`PackagesFixture { undo_expected: true }`; undo: history → `history info` →
pre-write rpm-факты → `dnf history undo <tid> -y` (erase-undo и install-undo
леги на фикстурах); `report_declares_transactional_undo` закрепляет
pkg.undo=transactional с агент-видимой пометкой; живые dnf-леги честно скипаются
вне Fedora.
Осталось: реальный Fedora-прогон, реальный откат пакета на disposable VM, demo3
на Fedora-леге, smoke свежих portal-интерфейсов Fedora — внешний шаг №5.

### Ф2.4. Manjaro T1

**2.4.1 — Manjaro-ветка: контракт T1 + своя тестовая матрица. Статус: частично.**
Доказательство: live на Manjaro GNOME/Wayland — полный T1-набор зелёный
(workspace, демо, 27/27 инструментов с грантами, заявленные прогоны);
`omnibus-probe/src/manjaro.rs` — ManjaroFacts, парсинг pacman.conf/mirrorlist;
live-баннер `manjaro(delayed_repos=true, mhwd=true, pamac=true)`; desktop-путь
mhwd-агностичен (AT-SPI2 over D-Bus, обход реального композитора);
`.github/workflows/manjaro.yml` — manjaro-контейнер, arch-fallback и
manjaro-vm-matrix (GNOME/KDE × X11/Wayland × mhwd non-free), включаемая
диспетчем при `vars.MANJARO_VM_ENABLED = true`.
Осталось: настоящие VM-ноги (KDE/X11-комбинации, smoke с non-free драйвером) —
внешний шаг №4; без включённых VM-раннеров матрица остаётся stand-in'ом
(классификатор лег в workflow честно печатает это).

### Ф2.5. doctor multi-distro и kb-guides

**2.5.1 — doctor: opt-in персистентности journald. Статус: частично.**
Доказательство (все автоматизируемые половины): volatile → opt-in-required с
РЕМЕДИЕЙ из обоих рычагов (грант + команда хранилища, ready_commands,
executed_by="human"); persistent → ok без команд; journald-адаптер пере-детектит
факт при каждом старте инстанса (проверка каталога /var/log/journal); prev_boot
появляется после гранта и перезагрузки (гейтинг в
`omnibus-core/tests/session_lifecycle.rs`, `tests/doctor_optin.rs`); согласие
гранулярное, вне контура агента; мутация — один аудит-событие с provenance
doctor (`applied_storage_mutation_is_one_audit_event` + продюсер-пин CLI);
отказ оставляет status quo без навязчивости (диагноз — чистая функция фактов:
`declined_opt_in_leaves_the_status_quo_without_nagging`).
Осталось: полный VM-проход с намеренно volatile Ubuntu и реальной перезагрузкой —
внешний шаг №1.

**2.5.2 — adapter-kb-guides: дистро-доки. Статус: частично.**
Доказательство: `omnibus-adapter-kb-guides/src/` — 24 теста: тот же контракт
KnowledgeBase, suite без изменений (`kb_suite_passes_against_the_offline_fixture`);
честность офлайна: stale только для транспортных сбоев и с пометкой `stale: true`,
док-левел промахи и чужие URL не маскируются, недостижимый источник без кэша —
честный отказ; выбор источника по семейству
(`kb_candidate_follows_the_knowledge_column` в `omnibusd/src/wiring.rs`).
Осталось: реальные зеркала дистро-док на Ubuntu-стенде; интеграционный тест
пары машин (один диагноз → разные статьи) — внешний шаг №10.

### Ф2.6. Каналы, QA-матрица, релиз v1.0

**2.6.1 — Упаковка: PPA, Debian repo, COPR. Статус: частично.**
Доказательство: `packaging/debian/` (control, rules, install, changelog,
source/format), `packaging/fedora/omnibus.spec`, `packaging/ppa/` (dput.cf с
allow_unsigned_uploads = 0, upload.sh через dpkg-buildpackage -S + dput,
README с posture build-хостов); `ci/validate_packaging.py` — 150 статических
проверок (включая подписную позу и оффлайн/флор-каналы), джобы package-build и
t2.matrix исполняют его; `package-install` на всех легах жёстко `req-ext`
(честный инвариант: контейнер не может честно заявить установку с рабочим
десктопом).
Осталось: публикация подписанных каналов (PPA upload, Debian repo trixie/
bookworm, COPR); проверка «свежая машина без доверия к ключу — честное
предупреждение» на каждом канале; установка штатным менеджером с TTFM < 5 мин
на каждой T2-платформе; upgrade MVP → v1.0 без ручных шагов — всё внешнее
(шаги №3, №4).

**2.6.2 — QA-матрица T2 и гейты tier. Статус: частично.**
Доказательство: `ci/matrix/t2.yml` — декларативные 8 платформ (arch, manjaro —
T1; debian-trixie, debian-bookworm, ubuntu-lts, mint, fedora — T2 smoke+contract;
debian-bullseye — T3 build-only) с честными правилами (stand-in для Mint,
bookworm desktop.* скрыты по флору at-spi2, REQ-EXT-инвариант package-install);
`.github/workflows/t2.yml` раскручивает матрицу из файла (container-legs) и
честно проваливает vm-legs («Not implemented — fail loudly instead of faking
the guarantee»); правило tier↔capabilities-таблицы сделано нормативным —
`docs/release-checklist.md` §4 п. 2 (формат строки расхождения, известные
кандидаты: bookworm desktop.*, apt pkg.undo=none, prev_boot за opt-in, NM за
netplan); демо №1–3 автоматизированы (`ci/smoke/`).
Осталось: скрипт демо №4 («С нуля») НЕ НАПИСАН — признано в ms2-acceptance,
это единственный отсутствующий кодовый артефакт Linux-части; реальные прогоны
матрицы (расписание t2.yml и включённые VM-леги); заполнение divergence-таблицы
(релизное время); demo4 на Arch и Ubuntu — внешний шаг №6.

**2.6.3 — Релиз v1.0. Статус: не начата.**
Доказательство отсутствия: `git tag` пуст (ни MVP-тега, ни v1.0), GitHub Release
нет, релизных заметок v1.0 нет, позиционирование/Show HN-фоллоу-ап/статья
Phoronix-Hackaday — нет. Готовы только процедурные заготовки:
`docs/release-checklist.md` §4 (содержимое отчёта) и §5 (порядок тега и анонсов),
шаблон формата — `docs/release-notes-0.1.0-draft.md`. Зависимость, которой нет:
DoD-наблюдение «в diff-логе omnibus-core с MVP только аддитивные расширения»
требует MVP-тега, который не проставлен (задача 1.8.3) — аудит формально
невыполним до релиза MVP. Сессионный прокси-аудит (git diff HEAD по omnibus-core:
4 файла, +138/−12, без изменений контракта) заявлен в ms2-acceptance — не
эквивалент формального тег-аудита.

---

## 4. Реестр требуемых решений

Сводка всех решений (ADR, правки спек/дизайн-доков, открытые M0-вопросы),
без которых соответствующие строки AC не закрыть. Нумерация Р-x.

**Обновление (ревизия 2026-10-17, вторая):** решения Р-1, Р-2, Р-3, Р-4, Р-5
(«флор остаётся») и Р-6 исполнены — реализация и затронутые артефакты
перечислены в разделе 0; Р-7 и Р-8 переданы в
`docs/verification/external-steps-runbook.md` как чисто организационные.
Ниже — исходные формулировки без изменений.

**Р-1. Формат значений полиформенных capabilities (pkg.undo).** AC 1.2.3 требует
«значение-enum», код передаёт JSON-строки (`"cache-downgrade"`, отсутствие =
none; `UNDO_VALUE` в `omnibus-adapter-alpm/src/lib.rs`). Варианты: (а) править
AC/спеку `specs/architecture/capability-model.md` под «строковое значение из
закрытого набора, проверяемого suite'ом»; (б) ввести enum в контракты и
перевести адаптеры. Затрагивает: 1.2.3 AC2. До решения: 1.2.3 = частично.

**Р-2. Граница доверия opaque-токенов.** AC 1.2.2 требует приватный конструктор
токенов («нельзя сконструировать вне ядра») — код предоставляет публичный
`MintKey::from_entropy_unchecked`, доверие держится на секретности ключа mint
ядра (честно задокументировано в `docs/verification/ms1-ms2-manjaro-acceptance.md`,
задача 1.2.2). Варианты: (а) ADR + правка AC/спеки (topology-and-lifecycle,
coupling-model) под фактическую модель; (б) сузить API (приватный конструктор в
пределах контрактов + core-only mint-фасад) и добавить негативный тест подделки.
Затрагивает: 1.2.2 AC2.

**Р-3. Ось GNOME/KDE в CI-матрице.** AC 1.4.1 и 1.8.2 требуют комбинаций
GNOME/KDE × X11/Wayland; в `ci.yml` ось DE сознательно не введена (обоснование
R7 в шапке файла: ни один шаг не ветвится по типу сессии; контейнерные леги
GNOME/KDE были бы байт-идентичны). Варианты: (а) править AC 1.4.1/1.8.2 в
роадмапе (и строку в `specs/architecture/platform-matrix.md`, если она
упоминает ось) с фиксацией критерия ввода оси; (б) ввести ось, добавив
шаг-потребитель (например, выбор композитора GNOME/KDE в headless-легах).
Затрагивает: 1.4.1 AC1, 1.8.2 AC4.

**Р-4. Буфер обмена: бэкенд и честность capabilities.** Как написано, AC 1.5.5
(«round-trip clipboard») через xdg-desktop-portal невыполним без
RemoteDesktop-сессии; How задачи называет связку «xdg-desktop-portal + wl-clipboard»,
но wl-clipboard/xclip в коде отсутствуют полностью (ни адаптер, ни probe их не
детектируют). Варианты: (а) реализовать wl-clipboard/xclip-бэкенд clip_read/
clip_write (код + capability `portal.clipboard` по факту наличия бинарников —
ровно как сформулировано в How) и тогда round-trip закрывается на десктоп-матрице;
(б) ADR + правка AC/спеки: v1 поставляет clipboard в статусе degraded с честным
AdapterUnavailable. Затрагивает: 1.5.5 AC1 (и частично 1.4.1 What — детекция
wl-clipboard/xclip в probe).

**Р-5. Флор AT-SPI2 и bookworm (открытый M0-элемент).** Факт: bookworm несёт
at-spi2 2.46 < флора 2.50 — на bookworm `desktop.*` скрыты (уже задокументировано
в §6 `docs/development/omnibus-architecture.md`, `specs/architecture/platform-matrix.md`,
`ci/matrix/t2.yml`, `docs/release-checklist.md` §2). Открытый вопрос: 5-DE sweep
(внешний замер) подтвердит 2.50 либо опустит флор (тогда bookworm получает
desktop.* обратно и маппинг платформ меняется). До замера текущий выбор —
«флор 2.50 стоит, bookworm T2-на-полу с документированной деградацией» — согласован
во всех документах. Затрагивает: 1.1.1 AC3, 2.1.2 (bookworm-лега).

**Р-6. Ubuntu 26.04 LTS в матрице.** m0-findings (таблица 2) зафиксировал новую
LTS (systemd 259, релиз 2026-04); спека и матрица ссылаются на 24.04. Решение:
входит ли 26.04 в T2-набор v1.0 (отдельная строка матрицы/леги) или остаётся за
границами обещаний v1.0. Затрагивает: 2.1.2, 2.6.2 (состав «8 платформ»).

**Р-7. Инфраструктура VM-раннеров.** manjaro-vm-matrix включается только
диспетчем при `vars.MANJARO_VM_ENABLED = true`, vm-legs в t2.yml проваливаются
намеренно до появления раннеров, Mint-лега — kind: vm. Решение: селф-хостед
VM-раннеры (какие, где, кто обслуживает) — без них T1-матрица Manjaro и T2-VM
гарантии (Mint, package-install) остаются REQ-EXT. Затрагивает: 2.4.1, 2.6.2,
1.8.2 (реальные прогоны).

**Р-8. Хостинг репозитория и branch protection.** «Красный main невозможен по
правилам репозитория» (AC 1.8.2) требует branch protection на реальном хостинге
— организационное решение, в репо не выражаемо.

**Р-9. Открытые M0-измерения (сводно, из чек-листа m0-findings).** Свежие
инсталляции 6 дистрибутивов (ls /var/log/journal) + первая перезагрузка
(list-boots ≥ 2); намеренно volatile-стенд (probe: persistent=false, prev_boot
отсутствует, doctor предлагает opt-in); 5-DE contract-suite sweep + gdbus-инспект
шины per DE per Wayland/X11 (решение Р-5); bullseye-негативный тест на VM.
Эти измерения — входы для решений Р-5 и подтверждений таблиц m0-findings.

ADR, требуемых «с нуля», два: Р-2 (если выбран вариант «зафиксировать фактическую
модель») и Р-4 (если выбран вариант «degraded без wl-clipboard»). Остальные
пункты — правки существующих документов (роадмап, capability-model,
platform-matrix, topology-and-lifecycle) либо организационные решения.

---

## 5. Реестр внешних шагов

Шаги, невыполнимые в настольном аудите и обязательные для закрытия Linux-части.
Сведение REQ-EXT-реестров обоих протоколов (6 + 10 пунктов) с устранением дублей
и добавлением выявленных аудитом пробелов. Порядок — логическая
последовательность.

**Блок A. Запуск реальной CI-инфраструктуры**

1. Первые реальные прогоны `.github/workflows/ci.yml` (gate, dependency-rules,
   matrix-t1 X11/Wayland, coverage, package-build) — все AC вида «CI гоняет …»
   сегодня опираются только на локальные прогоны тех же команд. (1.1.3, 1.1.4,
   1.8.2)
2. Замер времени gate на кэшированном раннере (< 10 мин, AC 1.1.3).
3. Branch protection + запрет красного main (AC 1.8.2; решение Р-8).
4. Расписание/запуск `.github/workflows/t2.yml`: контейнерные леги trixie,
   bookworm, ubuntu, fedora (`cargo test --workspace` + демо), bullseye
   build-only. (2.2.1, 2.6.2)
5. VM-раннеры + включение `MANJARO_VM_ENABLED`/`T2_VM_ENABLED` (решение Р-7):
   manjaro-vm-matrix (GNOME/KDE × X11/Wayland), Mint VM-лега.

**Блок B. VM-матрицы (функциональные подтверждения)**

6. Шесть дистрибутивов: probe выбирает корректные семейства; полы: bullseye —
   pkg скрыты с точной причиной, bookworm — полный T2 + статус «на полу»,
   Ubuntu/Mint — volatile-факт в живом документе. (2.1.1, 2.1.2)
7. Ubuntu VM с намеренно volatile journald: opt-in → грант + ручное создание
   хранилища → persistent=true → перезагрузка → prev_boot=true и чтение прошлой
   загрузки. (2.5.1; частично 1.5.2 — «тест на двух стендах»)
8. 5-DE AT-SPI2 sweep (GNOME/KDE/XFCE/Cinnamon/MATE, Wayland и X11): минимальная
   версия с зелёным контрактным набором → подтверждение/коррекция флора 2.50
   (решение Р-5); gdbus-инспект шины per DE. (1.1.1)
9. Десктоп-матрица чтения/записи: GNOME/KDE × X11/Wayland — tree, set_value,
   invoke, ru-раскладка, HiDPI-стенд, Electron-приложение, скриншот/notify;
   round-trip clipboard (в объёме решения Р-4). (1.4.1, 1.5.1, 1.5.5, 1.6.1)
10. Manjaro true-VM: KDE/X11-комбинации + smoke с non-free mhwd-драйвером. (2.4.1)
11. networkd-стенд и no-NM-стенд: скрытие сетевых инструментов без ошибок в
    журнале; прямой вызов → CapabilityRequired. (2.2.3)
12. Fedora: реальный `dnf history undo`-откат пакета на disposable VM; demo3 на
    Fedora-леге; smoke свежих portal-интерфейсов. (2.3.1)
13. Замер pkg_list/pkg_info на реальной apt-системе с ~2000 пакетов; живая
    apt-машина для схем с undo-пометкой. (2.2.1, 2.2.2)
14. Живой рестарт тестового юнита до active на реальной машине (буква AC 1.6.2);
    реальный downgrade из кэша на disposable VM (сознательно за границей
    автотестов). (1.6.2, 1.6.3)
15. kb-guides против реальных зеркал дистро-док на Ubuntu-стенде; интеграционный
    тест пары машин Arch/Ubuntu (один диагноз — разные статьи). (2.5.2)

**Блок C. Релиз MS1 (MVP, задача 1.8.3) — входная точка для MS2**

16. Публикация в AUR; установка `pacman -S omnibus` на чистой Arch-VM с замером
    TTFM < 5 мин; тест обновления с предыдущей версии. (1.8.1, 1.7.1)
17. `git tag` MVP-версии + GitHub Release с заметками из
    `docs/release-notes-0.1.0-draft.md`; Show HN + r/archlinux; заявка на
    a11y-трек GNOME/KDE; запуск north-star-трекинга. (1.8.3)

**Блок D. Каналы и релиз v1.0 (задачи 2.6.1–2.6.3)**

18. Написать скрипт демо №4 («С нуля») — единственный отсутствующий кодовый
    артефакт (`ci/smoke/` содержит только demo1–3). (2.6.2)
19. Публикация подписанных каналов: PPA (upload.sh), Debian repo (trixie/
    bookworm), COPR (Fedora); build-хосты с rustc ≥ 1.85 и vendored-деревом
    (posture в `packaging/README.md`).
20. Проверка на свежей машине без импортированного ключа: честное предупреждение
    диспетчера пакетов на каждом канале. (2.6.1)
21. Установка штатным менеджером на каждой T2-платформе: демон работает,
    TTFM < 5 мин; upgrade MVP → v1.0 без ручных шагов. (2.6.1)
22. Демо №1–4 зелёные на эталонных платформах (Arch и Ubuntu для demo4). (2.6.2)
23. `git tag v1.0.0`; формальный additive-only аудит diff-лога `omnibus-core`
    против MVP-тега (прокси уже заявлен, формальная процедура —
    `docs/release-checklist.md` §5). (2.6.3)
24. GitHub Release с отчётом: таблица 8 платформ по своим уровням,
    tier↔capabilities-таблица расхождений (включая bookworm desktop.*, apt
    pkg.undo=none, prev_boot за opt-in, NM за netplan), capabilities-документ
    эталонной машины, ограничения + never-лист, метрики TTFM по платформам и
    north-star в динамике; позиционирование Ubuntu/Mint/Fedora/Debian, Show HN
    фоллоу-ап, статья Phoronix/Hackaday. (2.6.2, 2.6.3)

---

## 6. Заявки существующих протоколов приёмки, не подтверждаемые кодом

Проверка выборочных заявок `docs/verification/ms1-ms2-manjaro-acceptance.md` и
`docs/verification/ms2-acceptance.md` против кода. Протоколы в массе добросовестны
(все 20+ цитируемых имён тестов существуют; признания о пробелах сделаны прямо),
но следующие заявки кодом не подтверждаются либо подтверждаются частично:

1. **1.2.2 AC2 «токены нельзя сконструировать вне ядра (приватный конструктор,
   негативный compile-тест)»** — протокол изначально пометил PASS, затем сам
   внёс исправление: приватного конструктора нет (`MintKey::from_entropy_unchecked`
   публичен), теста `forged_token_is_rejected` нет. AC роадмапа в этой редакции
   кодом не выполнен (решение Р-2).
2. **1.2.3 AC2 «pkg.undo типизирован enum'ом значений, а не строкой»** — протокол
   пометил PASS со ссылкой на «JSON-значения»; enum-типа в коде нет, значения —
   строки/отсутствие (решение Р-1).
3. **1.5.5 AC1 «round-trip clipboard … интеграционные тесты»** — протокол пометил
   PASS с оговоркой «clipboard round-trip сам по себе REQ-EXT»; фактически
   round-trip невозможен: clip_read/clip_write безусловно отвечают
   AdapterUnavailable (RemoteDesktop-гейтинг портала), фолбэка wl-clipboard нет.
   Требуемое AC поведение не существует (решение Р-4).
4. **1.4.1 AC1 / 1.8.2 AC4 «4 комбинации GNOME/KDE × X11/Wayland в CI»** —
   протоколы говорят о «headless proxy» и «matrix defined», но матрица в ci.yml
   однократная по оси (только X11/Wayland); требуемая AC матрица не определена
   даже в workflow (решение Р-3).
5. **1.8.2 AC1–3 «демо проходят как автотест» (грантованные леги)** — демо-скрипты
   существуют и покрывают read-леги и честные отказы, но грантованные write-леги
   (svc_start по гранту, set_value/invoke с грантом, реальный pkg_undo) в CI не
   исполняются — они делегированы юнит/интеграционным тестам ядра. Формулировка
   AC «проходит как автотест» в полном объёме не подтверждается; протокол это
   оговаривает («destructive GUI writes stay out of CI by design»).
6. **1.6.2 AC1 «рестарт тестового юнита доходит до активного состояния»** —
   живой рестарт не исполнялся; PASS основан на сентинел-семантике suite
   (omnibus-suite-slow) и честно оговорён. Буква AC на живой машине не
   подтверждалась (внешний шаг №14).
7. **1.8.1 «pacman -S omnibus (из AUR)» PASS-AS-PROXY** — AUR не публиковался;
   установка и upgrade-тест не выполнялись и не могли быть выполнены.
8. **Результаты прогонов E6 «462 passed», E7 «150 checks», coverage «77,63 %»,
   live-сессии E8–E10** — машинных артефактов в репозитории нет; существование
   заявленных тестов и проверок подтверждено кодом, но сами численные результаты
   чтением не воспроизводимы и в этом аудите не перепроверялись (настольный
   аудит; воспроизведение — прогон `make gate`, `make packaging`, `ci/smoke/run-demos.sh`).
9. **1.1.2 AC3 «проверено тестовым PR»** — реализовано как инлайн-негативная
   инъекция внутри CI-джобы (эквивалент механизма, не «тестовый PR»); локальное
   исполнение заявлено, CI-исполнение не подтверждено.
10. **ms2-acceptance: «demo4 … REQ-EXT»** — корректное признание, но важно
    зафиксировать: это не только «внешний прогон», а отсутствующий скрипт —
    кодовая работа (п. 18 раздела 5).

---

## 7. Итог: что нужно, чтобы закрыть Linux-часть

1. **Три кодовых/документных решения** (Р-1 pkg.undo-enum, Р-2 токены, Р-4
   clipboard-бэкенд) и одна правка AC (Р-3 ось DE) — либо кодом, либо поправкой
   роадмапа/спек; плюс два решения по мере готовности (Р-5 флор после sweep,
   Р-6 состав матрицы).
2. **Один отсутствующий кодовый артефакт** — скрипт демо №4.
3. **Инфраструктура**: реальные прогоны CI, branch protection, VM-раннеры
   (Manjaro-матрица, T2 VM-леги, Mint).
4. **VM-подтверждения** (блок B раздела 5): 6-дистрибутивная матрица, 5-DE sweep,
   десктоп-матрица, стенды networkd/no-NM, Fedora-откат, volatile-walkthrough.
5. **Релизы**: MVP (AUR + тег 0.1.0 + GitHub Release + анонсы) — входная точка;
   затем v1.0 (подписанные каналы + инсталляции/TTFM + demo4 + тег + additive-аудит
   + отчёт + анонсы).

Кодовое состояние Linux-части оценкой аудита: контракты, ядро, probe, 11
адаптеров, CLI, демо 1–3, упаковочные наборы и декларативная матрица — на месте
и согласованы между собой; критический путь до v1.0 лежит почти целиком во
внешних шагах и двух-трёх решениях, а не в недописанном коде.
