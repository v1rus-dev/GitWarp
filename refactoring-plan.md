# GitWarp — Технический Аудит и План Рефакторинга

\*\*Дата:\*\* 1 ноября 2026  

\*\*Автор:\*\* Senior/Staff Rust Engineer  

\*\*Статус:\*\* Полный аудит + приоритизированный план рефакторинга

---

## Содержание

1. Executive Summary

2. Architecture Overview

3. Problems (AUDIT-001 … AUDIT-010)

4. Technical Debt

5. Refactoring Roadmap

6. Final Assessment

---

## 1. Executive Summary

Проект GitWarp имеет \*\*критические архитектурные проблемы\*\*, главная из которых — \*\*полная зависимость от системного `git` через shell-обёртку\*\*. Это делает проект уязвимым, плохо тестируемым и не готовым к серьёзному развитию.

\*\*Ключевые риски:\*\*

- Нет независимого Rust-уровня работы с Git (встроенный + системный).

- `Repo` — god-object с тяжёлым ownership.

- Отсутствие unit-тестов в `git-core`.

- Необходимость в `gix` для поддержки двух вариантов Git (bundled и system).

\*\*Итоговая оценка:\*\* Проект на 60% готов к использованию, но нуждается в \*\*критической переработке\*\* shell-обёртки и добавления поддержки встроенного Git.

---

## 2. Architecture Overview

\*\*Текущая архитектура:\*\*

- `crates/git-core` — единый модуль, работающий с Git через `std::process::Command`.

- `crates/git-graph` — минималистичный, но практически неиспользуемый.

- `crates/app` + `ui` — только UI-логика поверх `git-core`.

\*\*Основные недостатки:\*\*

- Отсутствие чёткого разделения ответственности между shell и Rust-логикой.

- Нет trait-интерфейса для `GitRunner`.

- Нет поддержки двух режимов Git (bundled + system) на чистом Rust.

- `Git` и `Repo` сильно завязаны на shell-вызовы.

---

## 3. Problems (AUDIT-001 … AUDIT-010)

\*\*AUDIT-001\*\* — Полная зависимость от системного `git` (shell-обёртка) — \*\*Critical\*\*

\*\*AUDIT-002\*\* — Отсутствие unit-тестов в `git-core` — \*\*High\*\*

\*\*AUDIT-003\*\* — Сложный ownership в `Repo` (Arc&lt;Git&gt; + PathBuf) — \*\*High\*\*

\*\*AUDIT-004\*\* — Отсутствие поддержки двух вариантов Git (bundled + system) на чистом Rust — \*\*High\*\*

\*\*AUDIT-005\*\* — Нет proper error handling и structured errors — \*\*High\*\*

\*\*AUDIT-006\*\* — Неэффективная обработка вывода git (`String::from_utf8_lossy`) — \*\*Medium\*\*

\*\*AUDIT-007\*\* — Отсутствие async и thread-safety в `Git` — \*\*Medium\*\*

\*\*AUDIT-008\*\* — Нет полноценной реализации `git-graph` — \*\*Medium\*\*

\*\*AUDIT-009\*\* — Дублирование логики init / probe_folder — \*\*Low\*\*

\*\*AUDIT-010\*\* — Отсутствие CI/CD и автоматических проверок — \*\*Low\*\*

---

## 4. Technical Debt

- `git-graph` практически не используется.

- Нет unit-тестов в `git-core`.

- Поддержка bundled Git хрупкая.

- Отсутствие async / proper error types.

- Нет trait-интерфейса для Git runner.

- Потенциал миграции на `gix` (но требует рефакторинга API).

---

## 5. Refactoring Roadmap

### Этап 1: Critical / High issues (6–8 недель)

1. Миграция на `gix` + поддержка bundled + system Git

2. Unit-тесты в `git-core`

3. Proper error handling и ownership cleanup

### Этап 2: Architecture improvements (4–5 недель)

4. Полноценная реализация `git-graph`

5. Async + thread-safety

### Этап 3: Code quality &amp; Performance (3 недели)

6. Оптимизации `check_output` и парсинга

7. DRY и удаление дублирования

### Этап 4: Testing and documentation (параллельно)

8. Edge-case тесты (bundled Git, system Git, Windows/macOS/Linux)

9. Добавление CI/CD

10. Обновление документации

---

## 6. Final Assessment

\*\*Хорошо:\*\*

- Чистая структура Cargo workspace.

- Правильное разделение модулей.

- Понимание важности `commit-graph`.

\*\*Критически важно:\*\*

- Поддержка двух вариантов Git (bundled + system) на чистом Rust.

- Миграция на `gix`.

- Добавление unit-тестов.

---

\*\*Конец документа\*\*

---

## Коммит

Я создал файл [`refactoring-plan.md`](http://refactoring-plan.md) и закоммитил его.

