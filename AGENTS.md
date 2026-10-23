# AGENTS.md — сопровождение форка rakuyomi-mangalib

Инструкция для ИИ-агента (и человека), который поддерживает этот форк: обновляет его с upstream, собирает, тестирует и выпускает релизы. Пишите ответы и документацию на русском, команды и код оставляйте как есть.

Форк = несколько коммитов поверх upstream-тега [tachibana-shin/rakuyomi](https://github.com/tachibana-shin/rakuyomi). Он добавляет вход по токенам (access/refresh) для источников lib.social и собирается в **отдельный** плагин KOReader `rakuyomi-mangalib.koplugin`, который ставится рядом с оригинальным rakuyomi.

## 0. Жёсткие правила

1. **Токены никогда не попадают в репозиторий, коммиты, логи и вывод.** Не печатайте их (`echo`, `cat`, `print`, `set -x`, `env`), не вставляйте в тесты и issue. Передавайте только через переменные окружения или файл с правами `600` **вне репозитория**. Перед коммитом команда `git diff --cached | grep -E 'eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{150,}|def50200[0-9a-f]{100,}'` ничего не должна находить (короткие фикстуры и плейсхолдеры `eyJ…` / `def50200…` в тестах и подсказках — норма).
2. **Не форсить `main` без явного «да» человека.** `git push --force-with-lease` в `main` — только после подтверждения. Ветки-эксперименты можно пушить свободно.
3. **Не публиковать релизы без просьбы человека.** Тег `v*` сразу создаёт публичный релиз.
4. **Не называть в документации (README, INSTALL, NOTICE, AGENTS.md, release notes) конкретные источники, кроме MangaLib.** Остальные источники в `sources/` описывайте обобщённо: «источники lib.social». Это требование владельца. В коде названия остаются как есть.
5. **Инварианты side-by-side** (ломать нельзя, иначе плагин конфликтует с оригиналом):
   - папка плагина `rakuyomi-mangalib.koplugin`, внутреннее имя `rakuyomimangalib`, пункт меню «Rakuyomi MangaLib»;
   - каталог данных `koreader/rakuyomi-mangalib/`, сокет `/tmp/rakuyomi-mangalib.sock`;
   - префиксы настроек и Lua-модулей из `fork/side-by-side.json`;
   - самообновление отключено (`disable_self_update: true`): иначе плагин поставил бы оригинальный rakuyomi поверх форка;
   - архивы называются `rakuyomi-mangalib-<target>.zip`.
6. История — ровно логичные коммиты поверх upstream, без merge-коммитов. Исправления вливайте в нужный коммит через `git commit --fixup=<sha>` + `git rebase -i --autosquash <upstream-тег>`.
7. Не трогайте файлы с секретами, которые вам не дали явно (например, файлы паролей рядом с файлом токенов). Используйте только токены.

## 1. Устройство репозитория

### Коммиты форка (сверху вниз, поверх upstream-тега)

| Коммит | Что внутри |
|---|---|
| `feat(sources): bundle LibGroup/MangaLib Aidoku sources …` | `sources/libgroup` (общий код lib.social, авторизация в `src/auth/mod.rs`), `sources/ru.mangalib` |
| `fix(host): Aidoku host fixes …` | правки backend (см. таблицу ниже), `backend/shared/src/settings/linked.rs` |
| `fix(frontend): mask secret source settings …` | 3 Lua-файла upstream |
| `test: end-to-end token auth test …` | `sources/tests/` (мок-API + реальный server) |
| `feat(sources): … source with the same token auth` | второй источник lib.social в `sources/ru.*` |
| `build: package as separate side-by-side plugin …` | `fork/build.sh`, `fork/side_by_side.py`, `fork/side-by-side.json`, `fork/lua/`, `fork/build-sources.sh` |
| `docs, ci: …` | `README.md` (README форка вместо upstream), `fork/INSTALL_RU.md`, `NOTICE`, `.github/workflows/rakuyomi-mangalib.yml`, `fork/e2e/`, этот файл |

Посмотреть: `git log --oneline v1.44.2..HEAD` (подставьте текущий базовый тег; база записана в `NOTICE`).

### Только форковые файлы (конфликтов с upstream не бывает)

`NOTICE`, `.github/workflows/rakuyomi-mangalib.yml`, `fork/**`, `sources/**`, `backend/shared/src/settings/linked.rs`.

### Изменённые файлы upstream и зачем

| Файл | Зачем | Что делать при конфликте |
|---|---|---|
| `backend/server/src/app.rs` | импорт настроек из `<home>/import/<source_id>.json`, фоновый «персистер» значений `defaults.set` в `settings.json`, сброс live-зеркала при импорте | сохранить вызовы `import_source_settings`, `spawn_defaults_persister`, `linked::clear_live`; остальное брать из upstream |
| `backend/shared/src/chapter_downloader.rs` | вызов `get_page_list_for_chapter` (источнику нужен номер главы и тома) | заменить вызов `get_page_list(...)` на `get_page_list_for_chapter(token, chapter)` в новом коде |
| `backend/shared/src/source/mod.rs` | `get_page_list_for_chapter` / `get_page_list_with_info` | перенести обе функции рядом с новой `get_page_list` |
| `backend/shared/src/source/source_settings.rs` | сохранение значений источника, live-зеркало общих токенов (`note_source_loaded`, `live_value`, `publish_live`) | сохранить хуки в `new/get/all/save` |
| `backend/shared/src/source/wasm_imports/{defaults,net}.rs`, `next/{defaults,net}.rs` | `defaults.set` не печатает значение (токены!), `get_status_code` для next-SDK, отладка сети `RAKUYOMI_NET_DEBUG=1` (только имена заголовков) | **никогда** не возвращать печать значения в `defaults.set`. `next/*.rs` в upstream с CRLF: сохраняйте окончания строк |
| `backend/shared/src/settings/implementation.rs` | атомарная запись `settings.json` (tmp + rename) | оставить атомарную запись |
| `backend/shared/src/settings/mod.rs` | `mod linked;` | одна строка |
| `backend/shared/src/usecases/set_source_stored_settings.rs` | распространение токенов между связанными источниками | сохранить `clear_live` + `propagate_changes` |
| `frontend/rakuyomi.koplugin/SourceSettings.lua`, `widgets/SettingItemValue.lua` | маскировка `secret`-настроек (токены не видны на экране) | сохранить ветку `secret` |
| `README.md` | в upstream здесь английский README проекта; форк заменяет его русским README форка (единственный README форка) | **всегда оставлять версию форка**: во время rebase `git checkout --theirs README.md && git add README.md` (при rebase «theirs» — это коммит форка). Изменения upstream в README не переносить, только при необходимости обновить в нём базовую версию/sha |
| `AGENTS.md` | в upstream здесь английский гайд по проекту; форк заменяет его этим файлом | во время rebase взять версию форка: `git checkout --theirs AGENTS.md` (при rebase «theirs» — это коммит форка), затем прочитать `git diff $OLD $NEW -- AGENTS.md` и перенести в раздел 7 то, что важно |
| `frontend/rakuyomi.koplugin/Settings.lua` | `ffi.cdef` с уникальным именем `rakuyomi_sysinfo` (два плагина в одном Lua-процессе) | сохранить уникальное имя |

### Переименование при сборке

Исходники `frontend/rakuyomi.koplugin` остаются с upstream-именами. `fork/build.sh` собирает upstream-скриптами (`scripts/build-all.sh`, `scripts/build-plugin.sh`) обычный `build/rakuyomi.koplugin`, затем `fork/side_by_side.py` по правилам из `fork/side-by-side.json` делает копию `build/rakuyomi-mangalib.koplugin`: имя, меню, событие dispatcher, префиксы модулей и настроек, каталог данных, сокет, приоритет CBZ, отключённое самообновление, установка `bundled-sources/*.aix` (`fork/lua/BundledSources.lua`). **Каждое правило обязано сработать**, иначе сборка падает с понятной ошибкой. В конце скрипт проверяет, что в пакете не осталось `rakuyomi.koplugin`, `/tmp/rakuyomi.sock` и т. п. Если upstream переименовал строку, поправьте регулярку в `side_by_side.py` или `side-by-side.json`, а не upstream-файл.

### Общие токены

`linked.rs`: источники одного аккаунта lib.social (группа `LINKED_SOURCE_GROUPS`) делят ключи `accessToken`, `refreshToken`, `authJson`, `authClientId`. Обновлённая одним источником пара сразу видна остальным (live-зеркало) и записывается всем в `settings.json`. Ключ `separateAccount=true` в настройках источника отключает связь: у источника свои токены.

## 2. Обновление с upstream

Remotes: `upstream` = `https://github.com/tachibana-shin/rakuyomi`, `origin` = репозиторий владельца `git@github.com:korolr/rakuyomi-mangalib.git` (https://github.com/korolr/rakuyomi-mangalib), ветка форка = `main` (локально может называться `rakuyomi-mangalib`).

```sh
git remote -v                                        # проверить; если нет upstream:
git remote add upstream https://github.com/tachibana-shin/rakuyomi
git fetch upstream --tags
OLD=$(git describe --tags --abbrev=0 $(git merge-base HEAD upstream/main))   # текущая база, напр. v1.44.2
NEW=$(git tag -l 'v*' --sort=-v:refname | head -1)                          # последний upstream-тег
git switch main && git pull --ff-only origin main
git branch backup/pre-$NEW                           # точка отката
git rebase --onto "$NEW" "$OLD"
# конфликт: см. таблицу выше → правка → git add <файл> → git rebase --continue
# README.md / AGENTS.md: всегда версия форка → git checkout --theirs README.md AGENTS.md && git add README.md AGENTS.md
```

После rebase обязательно:

1. `fork/build.sh dev` (проверяет все правила переименования).
2. Полный набор проверок из раздела 3.
3. Обновить базу в `NOTICE` и `README.md` (версия + короткий sha).
4. Показать человеку `git log --oneline $NEW..HEAD` и `git range-diff $OLD..backup/pre-$NEW $NEW..HEAD`.
5. Получить согласие человека и выполнить `git push --force-with-lease origin main`. Без согласия пушьте в отдельную ветку: `git push origin HEAD:update/$NEW`.

Если upstream переписал затронутый файл целиком, перенесите правку заново по смыслу из таблицы, а не по diff. Если правка больше не нужна (upstream исправил сам), удалите её из коммита и упомяните это в release notes.

Обновление источников из Aidoku-Community: код `sources/libgroup` основан на `Aidoku-Community/sources` (`src/rust/ru.libgroup`). Переносите вручную, сохраняя `src/auth/`. После этого: `cd sources/libgroup && cargo test` и мок-тест.

## 3. Сборка и проверки

Все команды запускаются из корня репозитория. Перед ними один раз: `npm install && bash scripts/build-js-assets.sh`.

```sh
# backend
(cd backend && cargo fmt --check && cargo clippy --all -- -D warnings)
(cd backend && cargo test --all)                       # ~350 тестов
# источники (нужен aidoku-test-runner, см. раздел 6)
(cd sources/libgroup && cargo fmt --check && cargo test)
bash fork/build-sources.sh                             # → build/sources/*.aix
# мок-тест авторизации: настоящий server + мок API lib.social, без сети и реальных токенов
(cd backend && cargo build -p server) && python3 sources/tests/test_mangalib_auth.py -v
# Lua
luacheck frontend/
fork/build.sh dev && luacheck build/rakuyomi-mangalib.koplugin   # 3 предупреждения в platform/_meta.lua dev-сборки — норма
```

Сборка плагина (`cross` + podman, 10–15 минут на цель):

```sh
export CROSS_CONTAINER_ENGINE=podman
fork/build.sh kindle      # arm-unknown-linux-musleabi, подходит для любого Kindle
fork/build.sh kindlehf    # armhf, прошивка 5.16.3+
fork/build.sh kindlea9    # Cortex-A9
fork/build.sh aarch64     # 64-битные устройства
fork/build.sh desktop     # x86_64 musl, для KOReader на ПК
fork/build.sh dev         # быстро: debug-бинарник хоста, для e2e на сервере
fork/build.sh kindle --reuse   # упаковать заново, не пересобирая бинарники (только если backend не менялся!)
# результат: build/rakuyomi-mangalib-<target>.zip
unzip -l build/rakuyomi-mangalib-kindle.zip | grep -E 'server|bundled-sources|_meta'
```

Проверка в KOReader на сервере (e2e): скачайте Linux-сборку KOReader (`koreader-*-amd64.deb` со страницы релизов koreader/koreader), распакуйте `dpkg-deb -x ko.deb /opt/kotest/root`, затем:

```sh
fork/build.sh dev
KOREADER_DIR=/opt/kotest/root/usr/lib/koreader fork/e2e/koreader_run.sh
# Xvfb :2 поднимается сам; скриншот:
ffmpeg -loglevel error -f x11grab -video_size 600x800 -i :2 -frames:v 1 -y /tmp/shot.png
# нажатия: xdotool (DISPLAY=:2); лог: /tmp/kohome/ko.log
```

Критерий приёмки: в меню есть и «Rakuyomi», и «Rakuyomi MangaLib». Поиск в форке находит мангу, после нажатия глава скачивается и открывается. В `ko.log` есть `installed bundled source …` и сервер слушает `/tmp/rakuyomi-mangalib.sock`.

Версии: версия плагина берётся из тега (`SEMANTIC_RELEASE_VERSION` в CI), локально `1.0.0`. При изменении источника увеличьте `info.version` в `sources/<id>/res/source.json`. Плагин и так перезаписывает изменённые `.aix`, а номер версии нужен для наглядности в списке источников.

## 4. Релиз

Схема тегов: `v<upstream-версия>-mangalib.<N>`, например `v1.44.2-mangalib.1`. После rebase на новый upstream N снова начинается с 1.

1. Все проверки из раздела 3 зелёные, человек согласовал релиз.
2. Release notes на русском: `fork/release-notes/<тег>.md`. Что нового для пользователя, на какой upstream-версии основано, как обновиться (заменить папку плагина, настройки сохраняются). Без токенов, без названий источников кроме MangaLib. Закоммитьте их в последний (docs) коммит или отдельным коммитом с согласия человека.
3. `git tag -a v1.44.2-mangalib.1 -m "rakuyomi-mangalib 1.44.2-mangalib.1" && git push origin v1.44.2-mangalib.1`
4. CI `.github/workflows/rakuyomi-mangalib.yml`: `test` → `build` (матрица kindle, kindlehf, kindlea9, desktop, aarch64; `fork/build.sh <target>` с версией из тега) → `release` (softprops/action-gh-release: все `rakuyomi-mangalib-*.zip` + `SHA256SUMS.txt`, текст из `fork/release-notes/<тег>.md`, если файла нет — короткая заглушка).
5. Проверка: `gh release view <тег>` (или страница Releases). Должно быть 5 zip и SHA256SUMS. Скачайте kindle-архив, выполните `sha256sum -c`, `unzip -l`, проверьте `_meta.lua` (`rakuyomimangalib`) и версию в `BUILD_INFO.json` пакета.

**Upstream-воркфлоу в репозитории владельца должны быть отключены** (Actions → workflow → Disable или `gh workflow disable "<имя>"`): `build.yml` на push в `main` запускает semantic-release и создаёт свои теги и релизы; `cookie-bot.yml` и `deploy-pages.yml` тоже рассчитаны на upstream. Отключить нужно все пять: «Build», «CI», «Cookie Bot», «Deploy mdBook to GitHub Pages», «luacheck». Включённым остаётся только «rakuyomi-mangalib». Проверка: `gh workflow list --all`. Никогда не делайте `git push --tags` в origin: upstream-теги `v1.*` запустили бы релизы форка.

## 5. Проверка с реальными токенами

Токены лежат в файле вне репозитория, например `~/.secrets/mangalib_tokens.json` (`chmod 600`, формат `{"access_token": "...", "refresh_token": "..."}`), или в переменных `MANGALIB_ACCESS_TOKEN` / `MANGALIB_REFRESH_TOKEN`. Их выдаёт человек: войти на mangalib.me, взять из localStorage пару `auth` (access + refresh).

```sh
fork/build.sh dev
python3 fork/e2e/live_tokens.py --secrets ~/.secrets/mangalib_tokens.json               # поиск 18+ тайтла + скачивание
python3 fork/e2e/live_tokens.py --anonymous --case "ru.mangalib.tokens:Solo Leveling"   # без токенов
python3 fork/e2e/live_tokens.py --secrets ~/.secrets/mangalib_tokens.json --force-refresh   # проверка refresh (ротирует пару!)
```

Скрипт редактирует токены в выводе, проверяет, что их нет в логе сервера, и после ротации записывает новую пару обратно в `--secrets`.

Особенности API lib.social (важно для отладки):

- **Refresh ротирует обе части пары.** `POST {api}/api/auth/oauth/token` с `{"grant_type":"refresh_token","client_id":…,"refresh_token":…,"scope":""}` возвращает новые access и refresh (срок жизни обоих 31 день). Старые сразу отзываются: старый refresh даёт `401 {"hint":"Token has been revoked"}`, старый access перестаёт работать. **Сессия браузера, из которой взяли пару, разлогинивается.** Каждый `--force-refresh` тратит пару: новая обязана быть сохранена, иначе нужен повторный вход человека. Не запускайте refresh без нужды и никогда не удаляйте временный home, если в нём единственная копия новой пары (скрипт оставляет home с правами 700 и пишет путь).
- `client_id` берётся из поля `aud` JWT access-токена. Если не подошёл, плагин пробует 1, 3, 2.
- Запросу нужны браузерные заголовки (`User-Agent`, `Referer`, `Origin`, `Site-Id`), иначе Cloudflare вернёт 403.
- **Отозванный или неверный токен на контентных эндпоинтах молча работает как аноним** (никакого 401). Только `/api/auth/me` честно отвечает 401. Проверяйте авторизацию по 18+ контенту: например, `GET https://api.cdnlibs.org/api/manga/7072--eleceed-` (заголовок `Site-Id: 1`) анонимно даёт 404, с токеном 200.
- Плагин обновляет токен заранее (за 120 с до `exp`) и повторно при 401. После неудачи refresh не повторяется 10 минут. Отозванный refresh не перебирается по client_id.
- Отладка сети: `RAKUYOMI_NET_DEBUG=1` печатает `[net] METHOD url -> status headers=[имена]`, без значений.

## 6. Настройка сервера Ubuntu (24.04)

```sh
sudo apt update
sudo apt install -y git curl build-essential pkg-config libfontconfig1-dev libfreetype6-dev \
  zip unzip python3 podman uidmap gettext lua-check rsync ca-certificates
# для e2e в KOReader (по желанию): xvfb xdotool x11-utils ffmpeg
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustup toolchain install stable --component rustfmt clippy
rustup target add wasm32-unknown-unknown
curl -fsSL https://bun.sh/install | bash -s "bun-v1.3.14" && export PATH="$HOME/.bun/bin:$PATH"
sudo apt install -y nodejs npm            # для `npm install` (semantic-release-зависимости upstream)
cargo install cross
cargo install --git https://github.com/Aidoku/aidoku-rs --rev ae6485e3c14ecd1fd5497d170eae884c68d04a87 aidoku-test-runner
echo 'export CROSS_CONTAINER_ENGINE=podman' >> ~/.bashrc
podman run --rm docker.io/library/alpine true   # проверка rootless podman
```

Затем в клоне репозитория: `npm install && bash scripts/build-js-assets.sh` и проверки из раздела 3. CI делает то же самое (см. `.github/workflows/rakuyomi-mangalib.yml`).

## 7. Архитектура upstream (кратко)

Оригинальный английский гайд upstream по проекту: `git show upstream/main:AGENTS.md`. Читайте его перед изменениями в backend или frontend. Главное:

- `backend/` — Rust workspace: `server/` (axum HTTP-сервер; на Kindle общается с плагином через Unix-сокет), `shared/` (модели, SQLite через sqlx, менеджер источников на wasmi, загрузчик, настройки), `uds_http_request/`, `cbz_metadata_reader/`, `wasm_macros/`, `wasm_shared/`.
- `frontend/rakuyomi.koplugin/` — Lua-плагин: `Backend.lua` (HTTP/JSON к серверу), `main.lua` (меню, Dispatcher), `platform/` (TCP или UDS + fork/exec), UI-виды, `jobs/`, `l10n/`.
- Edition 2021, stable toolchain, `anyhow` в бинарниках, `thiserror` в библиотеках, tokio. Release-профиль: `lto="fat"`, `panic="abort"`.
- Источники Aidoku: WASM (legacy SDK и next SDK `aidoku-rs`). Импорты хоста — `backend/shared/src/source/wasm_imports/` (next SDK — в `next/`).
