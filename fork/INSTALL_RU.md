# Rakuyomi MangaLib на Kindle: установка и токены MangaLib

Архив `rakuyomi-mangalib-kindle.zip` содержит полностью статические ARM-бинарники (soft-float ABI, без зависимостей от системных библиотек), как и сборка `kindle` оригинального rakuyomi. Они запускаются на Kindle с джейлбрейком и KOReader, в том числе на прошивках 5.16.3+ с hard-float.

Плагин ставится **рядом** с оригинальным rakuyomi и ничего у него не трогает. У него своя папка, свои данные в `koreader/rakuyomi-mangalib/`, свой сокет `/tmp/rakuyomi-mangalib.sock` и свои ключи настроек.

## 1. Установка

1. Подключите Kindle к компьютеру по USB.
2. Распакуйте `rakuyomi-mangalib-kindle.zip` в папку `koreader/plugins/` на Kindle. Должно получиться так:
   ```
   koreader/plugins/rakuyomi-mangalib.koplugin/main.lua
   koreader/plugins/rakuyomi-mangalib.koplugin/server
   koreader/plugins/rakuyomi-mangalib.koplugin/bundled-sources/ru.mangalib.tokens.aix
   ```
   На устройстве это путь `/mnt/us/koreader/plugins/rakuyomi-mangalib.koplugin/`. Папку `rakuyomi.koplugin`, если она есть, не трогайте.
3. Отключите Kindle и перезапустите KOReader: выйдите из него и откройте снова.
4. В меню KOReader, на вкладке с поиском и инструментами, появится пункт **Rakuyomi MangaLib**.

При первом открытии плагин сам установит источник **«MangaLib (токены)»**. Поиск и скачивание открытых тайтлов в нём работают и без токенов.

## 2. Получение токенов

1. На компьютере войдите в аккаунт на https://mangalib.me.
2. Нажмите F12, откройте вкладку **Console** и выполните:
   ```js
   copy(JSON.stringify(JSON.parse(localStorage.auth).token))
   ```
   В буфере обмена окажется JSON вида `{"token_type":"Bearer","expires_in":...,"access_token":"eyJ...","refresh_token":"def50200...",...}`. Плагин принимает его как есть.

   Токены по отдельности (для способа Б): `copy(JSON.parse(localStorage.auth).token.access_token)` и `copy(JSON.parse(localStorage.auth).token.refresh_token)`. Refresh token очень длинный, поэтому копируйте его целиком, без обрезки.
3. Никому не отправляйте токены: с ними есть полный доступ к аккаунту.

При первом обновлении токенов плагином старая пара отзывается, и браузер, из которого вы её взяли, разлогинится. Это нормально: войдите на сайт заново, плагину это не помешает.

## 3. Передача токенов плагину (любой способ)

**Способ А: файлом (проще всего).**
1. Подключите Kindle по USB.
2. Создайте папку `koreader/rakuyomi-mangalib/import/`, если её ещё нет.
3. Создайте в ней файл `ru.mangalib.tokens.json` и вставьте туда JSON из буфера. Полный путь на устройстве: `/mnt/us/koreader/rakuyomi-mangalib/import/ru.mangalib.tokens.json`.
4. Отключите Kindle, перезапустите KOReader и откройте Rakuyomi MangaLib.

Подходит JSON из команды выше, весь `localStorage.auth` (`{"token": {...}, "auth": {...}}`) или `{"accessToken": "...", "refreshToken": "..."}`. Токены попадут в настройки источника, а файл удалится автоматически.

**Способ Б: вручную.**
1. Откройте Rakuyomi MangaLib и нажмите шестерёнку слева вверху → **Manage sources** → **MangaLib (токены)**.
2. Вставьте значения в поля **Access token** и **Refresh token**. В поле Access token можно вставить и весь JSON из команды выше, тогда Refresh token можно не заполнять.

Сохранённые токены отображаются маской `•••• (длина)`.

## 4. Как это работает дальше

- Access token обновляется сам: незадолго до истечения и при ответе 401. Новая пара токенов сразу записывается в `koreader/rakuyomi-mangalib/settings.json`.
- Если сервер отклонил refresh token, повторная попытка будет не раньше чем через 10 минут, а до тех пор источник работает анонимно.
- Если на сайте выполнить «выход из всех устройств», refresh token перестанет действовать. Чтобы вернуть вход, повторите шаги 2–3.

## 5. Использование

Rakuyomi MangaLib → значок поиска → введите название → **Search** → выберите тайтл → нажмите на главу. Глава скачается и откроется. Скачанные главы лежат в `koreader/rakuyomi-mangalib/downloads/`. Платные главы раннего доступа помечены как закрытые и не скачиваются.

## Если что-то не так

- Пункта меню нет: проверьте, что путь именно `koreader/plugins/rakuyomi-mangalib.koplugin/main.lua`, без лишней вложенной папки.
- Сервер не запускается: проверьте, что у `server` остались права на исполнение. Можно перераспаковать архив или выполнить через SSH `chmod +x /mnt/us/koreader/plugins/rakuyomi-mangalib.koplugin/{server,uds_http_request,cbz_metadata_reader}`.
- Лог KOReader лежит в `/mnt/us/koreader/crash.log`. Токены в лог не пишутся.
