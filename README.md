# rakuyomi-mangalib

Форк [rakuyomi](https://github.com/tachibana-shin/rakuyomi) — плагина для чтения манги в [KOReader](https://github.com/koreader/koreader). Форк добавляет полноценный вход в **MangaLib** по access/refresh-токенам. Плагин ставится **отдельно, рядом** с оригинальным rakuyomi и ничего в нём не меняет.

Готовые сборки: [Releases](https://github.com/korolr/rakuyomi-mangalib/releases).

## Зачем этот форк

rakuyomi умеет работать с источниками Aidoku, в том числе с MangaLib. Но войти в аккаунт MangaLib на Kindle нельзя: в Aidoku вход сделан через встроенный браузер, а в KOReader браузера нет. Без входа MangaLib показывает не всё. Тайтлы 18+ и часть других тайтлов анонимно не открываются: сайт отвечает «не найдено».

Что делает форк:

- **Вход по токенам.** Пара `access_token` / `refresh_token` копируется из браузера на компьютере одной командой и передаётся плагину файлом или через настройки. После этого поиск, карточки и главы, включая 18+, запрашиваются от имени вашего аккаунта.
- **Автообновление токенов.** Access token живёт ограниченное время. Плагин сам обновляет его незадолго до истечения и при ответе 401. MangaLib при каждом обновлении выдаёт новый refresh token, и новая пара сразу сохраняется. Поэтому вход переживает перезапуски KOReader, и токены не нужно копировать заново.
- **Защита аккаунта.** Сохранённые токены показываются в настройках маской `•••• (длина)` и не пишутся в логи. Если сервер отклонил refresh token, повторная попытка будет не раньше чем через 10 минут: плагин не засыпает сервер авторизации запросами.
- **Отдельный плагин рядом с оригиналом.** Папка `rakuyomi-mangalib.koplugin`, пункт меню «Rakuyomi MangaLib», данные в `koreader/rakuyomi-mangalib/`, свой сокет и свои настройки. Оригинальный rakuyomi можно держать установленным одновременно, они друг другу не мешают.
- **Сборки для Kindle.** Полностью статические ARM-бинарники с теми же целями, что у upstream: `kindle` (подходит для всех Kindle с джейлбрейком), `kindlehf`, `kindlea9`. Есть также `aarch64` и `desktop`.
- **Источник уже внутри.** Источник «MangaLib (токены)» лежит в плагине и устанавливается сам при первом запуске. Списки источников подключать не нужно.

Самообновление плагина в форке отключено, иначе оно скачало бы оригинальный rakuyomi. Новые версии берите со страницы релизов.

## Установка (Kindle с джейлбрейком и KOReader)

1. Скачайте `rakuyomi-mangalib-kindle.zip` со страницы [Releases](https://github.com/korolr/rakuyomi-mangalib/releases).
2. Подключите Kindle по USB и распакуйте архив в `koreader/plugins/`. Должно получиться так:
   ```
   koreader/plugins/rakuyomi-mangalib.koplugin/main.lua
   koreader/plugins/rakuyomi-mangalib.koplugin/server
   koreader/plugins/rakuyomi-mangalib.koplugin/bundled-sources/ru.mangalib.tokens.aix
   ```
   Папку `rakuyomi.koplugin`, если она есть, не трогайте.
3. Отключите Kindle и перезапустите KOReader. В меню, на вкладке с поиском и инструментами, появится пункт **Rakuyomi MangaLib**.

При первом открытии плагин сам установит источник **«MangaLib (токены)»**. Открытые тайтлы ищутся и скачиваются и без токенов.

Подробная инструкция с разбором проблем: [`fork/INSTALL_RU.md`](fork/INSTALL_RU.md).

## Как получить токены

1. На компьютере войдите в аккаунт на https://mangalib.me.
2. Нажмите F12, откройте вкладку **Console** и выполните:
   ```js
   copy(JSON.stringify(JSON.parse(localStorage.auth).token))
   ```
   В буфере обмена окажется JSON вида `{"token_type":"Bearer","expires_in":…,"access_token":"eyJ…","refresh_token":"def50200…",…}`. Плагин принимает его как есть.

   Если нужны токены по отдельности:
   ```js
   copy(JSON.parse(localStorage.auth).token.access_token)    // access token
   copy(JSON.parse(localStorage.auth).token.refresh_token)   // refresh token
   ```
   Их же можно найти вручную: **Application → Local Storage → https://mangalib.me → `auth`**, поле `token`. Refresh token очень длинный, копируйте его целиком.
3. Никому не отправляйте токены: с ними есть полный доступ к аккаунту.

Важно: при первом же обновлении токенов плагином старая пара отзывается, поэтому браузер, из которого вы их взяли, разлогинится. Это нормально, просто войдите на сайт заново. Ваш новый вход и плагин друг другу не мешают. Не нажимайте на сайте «Выйти на всех устройствах», иначе плагин тоже потеряет вход.

## Как передать токены плагину

**Способ А: файлом (удобнее всего).**
1. Подключите Kindle по USB и создайте папку `koreader/rakuyomi-mangalib/import/`. Полный путь на устройстве: `/mnt/us/koreader/rakuyomi-mangalib/import/`.
2. Создайте в ней файл `ru.mangalib.tokens.json` и вставьте в него содержимое буфера.
3. Отключите Kindle, перезапустите KOReader и откройте Rakuyomi MangaLib.

Подходит любой из форматов:
- JSON из команды выше (`{"access_token": "…", "refresh_token": "…", …}`);
- весь `localStorage.auth` (`{"token": {…}, "auth": {…}}`);
- `{"accessToken": "…", "refreshToken": "…"}`.

При запуске сервер переносит токены в настройки источника и **удаляет файл**.

**Способ Б: в настройках.** Откройте Rakuyomi MangaLib и выберите шестерёнку → **Manage sources** → **MangaLib (токены)**. Затем заполните поля **Access token** и **Refresh token**. В поле Access token можно вставить и весь JSON из команды выше: тогда поле Refresh token можно не трогать.

Если refresh token перестал действовать (например, после «выхода на всех устройствах»), источник продолжит работать анонимно. Чтобы вернуть вход, получите токены заново и передайте их любым способом.

## Использование

Rakuyomi MangaLib → значок поиска → введите название → **Search** → выберите тайтл → нажмите на главу. Глава скачается и откроется. Скачанные главы лежат в `koreader/rakuyomi-mangalib/downloads/`. Платные главы раннего доступа помечены как закрытые и не скачиваются.

## Сборка

Нужны Rust (stable), `rustup target add wasm32-unknown-unknown`, `cross` с podman или docker (для ARM), bun, Node.js/npm, gettext, zip и python3. Полный список пакетов для Ubuntu 24.04 — в [`AGENTS.md`](AGENTS.md), раздел 6.

```sh
npm install
fork/build.sh kindle            # полная сборка → build/rakuyomi-mangalib-kindle.zip
fork/build.sh kindle --reuse    # переиспользовать уже собранные backend/target/<target>/release
fork/build.sh dev               # отладочная сборка для хоста
fork/build-sources.sh           # только источники → build/sources/*.aix
```

Тесты:

```sh
(cd backend && cargo test --all)
(cd sources/libgroup && cargo test)              # нужен aidoku-test-runner
(cd backend && cargo build -p server) && fork/build-sources.sh && python3 sources/tests/test_mangalib_auth.py
```

`test_mangalib_auth.py` запускает настоящий сервер с настоящим `.aix` против мок-API. Он проверяет обновление токенов, повтор после 401, ротацию, сохранение пары и импорт файлом. Аккаунт для этого не нужен.

CI: `.github/workflows/rakuyomi-mangalib.yml` запускается на каждый push. Тег `v*` (например `v1.44.2-mangalib.1`) создаёт релиз с архивами `rakuyomi-mangalib-<target>.zip` и `SHA256SUMS.txt`. Файлы upstream-воркфлоу не изменены, но в репозитории форка их нужно отключить в настройках Actions (см. [`AGENTS.md`](AGENTS.md), раздел 4).

## Как обновиться с upstream

История форка — это несколько коммитов поверх upstream-тега. Upstream-файлы почти не меняются: переименование плагина делается **во время сборки** скриптом `fork/side_by_side.py` по правилам из `fork/side-by-side.json`. Поэтому обновление сводится к `git rebase`:

```sh
git remote add upstream https://github.com/tachibana-shin/rakuyomi   # один раз
git fetch upstream --tags
git switch main
git branch backup/pre-v1.45.0                 # резервная копия
git rebase --onto v1.45.0 v1.44.2             # --onto <новый upstream-тег> <старый upstream-тег>
# при конфликтах: поправить, git add …, git rebase --continue
# README.md и AGENTS.md при конфликте всегда берутся из форка: git checkout --theirs README.md AGENTS.md
fork/build.sh dev                             # side_by_side.py упадёт, если правило переименования перестало срабатывать
(cd backend && cargo test --all) && python3 sources/tests/test_mangalib_auth.py
git push --force-with-lease origin main
```

Подробная инструкция для сопровождения, в том числе для ИИ-агента, — в [`AGENTS.md`](AGENTS.md).

На что обратить внимание:

- `fork/side_by_side.py` проверяет каждое правило замены. Если upstream переименовал файл или строку, сборка остановится с понятной ошибкой: поправьте правило в скрипте. В конце скрипт проверяет, что в пакете не осталось ссылок на `rakuyomi.koplugin`, `/tmp/rakuyomi.sock` и подобные пути.
- Конфликты чаще всего возникают в `backend/shared/src/source/wasm_imports/` и `backend/server/src/app.rs` (это исправления хоста).
- Файлы `wasm_imports/next/net.rs` и `defaults.rs` в upstream записаны с CRLF. Не меняйте окончания строк, иначе diff станет огромным.
- Источники в `sources/` обновляются отдельно, из Aidoku-Community (`libgroup` и `ru.mangalib`).

## Лицензия и авторство

- Оригинальный проект: **[rakuyomi](https://github.com/tachibana-shin/rakuyomi)** © [hanatsumi](https://github.com/hanatsumi), [tachibana-shin](https://github.com/tachibana-shin) и контрибьюторы. Лицензия [GNU AGPL-3.0](LICENSE). Форк распространяется под той же лицензией. Документацию оригинала смотрите в [его репозитории](https://github.com/tachibana-shin/rakuyomi#readme) и в папке [`docs/`](docs/).
- Изменения форка перечислены в [`NOTICE`](NOTICE). Основа — upstream 1.44.2 (коммит `6e61856`).
- Код в `sources/` взят из [Aidoku-Community/sources](https://github.com/Aidoku-Community/sources) под лицензией MIT или Apache-2.0 (`sources/LICENSE-MIT`, `sources/LICENSE-APACHE`). Изменения: вход по токенам, автообновление, сборка под rakuyomi.
- Используется SDK [aidoku-rs](https://github.com/Aidoku/aidoku-rs).
- MangaLib — сервис сторонних правообладателей. Форк с ним не связан.

По AGPL-3.0 при распространении сборок вы обязаны предоставить исходный код. Для этого достаточно ссылки на этот репозиторий.
