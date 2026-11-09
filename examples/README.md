# Lust Language - Examples & Guides

Комплексный пакет наглядных, запускаемых примеров (Effect-TS / ZIO стиль) для **Effect System v2.0** и **Testing Framework (`@std/test`)** в языке Lust.

---

## 📚 Каталог примеров

Все примеры доступны в директориях [`examples/effects/`](./effects/) и [`examples/testing/`](./testing/).

| # | Пример | Файл | Команда запуска | Описание |
|---|---|---|---|---|
| **01** | **Basic Effects & Exit** | [`01_basic_effects.rs`](./effects/01_basic_effects.rs) | `cargo run --example effects_basic` | Базовые абстракции `Exit<A, E>`, обработка ошибок `Cause::Fail`, фатальных сбоев `Cause::Die` и прерываний `Cause::Interrupted`. Монадический `and_then`. |
| **02** | **Retry & Repeat Policies** | [`02_retry_and_repeat.rs`](./effects/02_retry_and_repeat.rs) | `cargo run --example effects_retry_repeat` | Алгебра политик расписаний `ScheduleKind` (recurs, exponential backoff с jitter). Проверка инварианта fail-closed: прерывания `OutOfMemory`/`OutOfGas` никогда не перезапускаются. |
| **03** | **Safe Resource Scopes** | [`03_acquire_release_and_scopes.rs`](./effects/03_acquire_release_and_scopes.rs) | `cargo run --example effects_acquire_release` | Идиома `acquire_release` с маскированием прерываний во время захвата ресурса (`enter_mask`/`exit_mask`) и регистрация финализатора в `Scope`. |
| **04** | **Virtual Clock Testing** | [`04_testing_with_virtual_clock.rs`](./effects/04_testing_with_virtual_clock.rs) | `cargo run --example 04_testing_with_virtual_clock` | Юнит-тестирование эффектов с виртуальным временем `TestClockState`. Симуляция экспоненциального бэкоффа (>120 секунд виртуального времени) за **< 1 мс** реального времени без вызовов `std::thread::sleep`. |
| **05** | **Mocking & Stack Projection** | [`05_dependency_injection_mocking.rs`](./effects/05_dependency_injection_mocking.rs) | `cargo run --example 05_dependency_injection_mocking` | Подмена зависимостей через `MockService`, регистрация вызовов (call spies), stub-ответы и безаллокационная стековая проекция окружения `StackContextProjection<N>` (Zero-Heap DI). |
| **06** | **Structured Concurrency** | [`06_concurrency_and_supervision.rs`](./effects/06_concurrency_and_supervision.rs) | `cargo run --example 06_concurrency_and_supervision` | Structured Concurrency: дерево файберов в `Scope`, автоматическая каскадная отмена дочерних задач при сбое и **Gate-NEG-02** — запрет освобождения хост-ресурсов до завершения всех дочерних задач с последующей LIFO финализацией. |
| **07** | **Lust Test Syntax Demo** | [`07_test_syntax_demo.lust`](./effects/07_test_syntax_demo.lust) | Исходный код на Lust | Нативный синтаксис модуля `@std/test` / `std.test`: `suite`, `describe`, `test`, модификаторы `test.skip(...)`, `test.timeout(...)`, хуки `before_each`/`after_each`/`before_all`/`after_all` и ассерты эффектов. |

---

## 🚀 Запуск и проверка

Каждый пример компилируется и выполняется автономно:

```bash
# 1. Virtual Clock Testing (zero-delay simulation)
cargo run --example 04_testing_with_virtual_clock

# 2. Dependency Injection & Mocking (StackContextProjection)
cargo run --example 05_dependency_injection_mocking

# 3. Structured Concurrency, Fiber Supervision & Gate-NEG-02
cargo run --example 06_concurrency_and_supervision
```

Для проверки всего набора тестов компилятора и тестового фреймворка:

```bash
cargo test --test std_test_framework
```

---

## 🔍 Архитектурные детали

### 1. Виртуальное время (`TestClockState`)
Позволяет тестировать сложные сетевые retry-циклы, экспоненциальные задержки и таймауты детерминированно. Вся очередь таймеров упорядочена в min-heap (`BinaryHeap<Reverse<ScheduledTimer>>`), а метод `adjust(delta_ms)` мгновенно пробуждает файберы, время которых наступило.

### 2. Zero-Heap StackContextProjection
Представление контекста окружения (`StackContextProjection<N>`) как плоского стекового массива указателей и тегов служб (`[*const c_void; N]`, `[u32; N]`). Полностью исключает динамические аллокации в куче при резолвинге сервисов в горячих путях эффектов.

### 3. Инвариант Gate-NEG-02 (Structured Concurrency Teardown)
При завершении или сбое в области видимости (`Scope`):
1. **Admission Closed**: немедленно блокируется регистрация новых дочерних задач и ресурсов (`ScopeAdmission::Closed`).
2. **Cascading Cancellation**: всем активным файберам рассылается сигнал прерывания `InterruptReason::Cancelled`.
3. **Joining Barrier**: область видимости переходит в фазу `ScopeTeardown::Joining` и ожидает остановки всех дочерних задач. Ни один хост-ресурс **не финализируется**, пока файберы активны или находятся в маскированных критических секциях.
4. **LIFO Finalization**: после присоединения всех задач запускаются финализаторы строго в обратном порядке (LIFO).
