# compact-terminal (`ct`) - plan

## Potwierdzone ustalenia

- Nazwa binarki/projektu: `ct`
- Narzedzie wywoluje komendy z terminala
- Komunikacja: MCP przez `stdio`
- Implementacja: Rust + Cargo
- Dane runtime i logi: `~/.ct`

## Cel

Przygotowac `ct` jako narzedzie MCP do uruchamiania komend terminalowych oraz przechowywania informacji o uruchomionych procesach i ich logow w `~/.ct`.

## Cel produktu (AI + MCP proxy)

- Glownym celem narzedzia jest bycie MCP proxy do terminala dla AI.
- `ct` ma kontrolowac i standaryzowac wykonywanie komend od agenta AI.
- `ct` ma ograniczac szum w odpowiedziach, aby zmniejszac zuzycie okna kontekstu.

## Kluczowe funkcje produktu

- Weryfikacja bezpieczenstwa komendy przed uruchomieniem (policy check / guardrails).
- Logowanie uruchomionych komend i ich wynikow.
- Redukcja outputu dla AI (np. dla builda zwrot skrocony: `success` / `failed` + metadata).
- Pelny zapis logow wykonania metody/operacji w `~/.ct/logs`.
- Mozliwosc odczytu, filtrowania i przeszukiwania logow.
- Mozliwosc zarzadzania procesami (status, lista, ubicie procesu, cleanup).

## Funkcje konkurencyjne (priorytety)

### Must-have (konkurencyjne MVP)

- Policy engine przed wykonaniem komendy (`allow` / `deny` / `ask`) z uzasadnieniem decyzji.
- Deterministyczny audit log: komenda, `cwd`, timestamp, exit code, duration, `run_id`.
- Domyslny tryb `compact` dla AI: `success` / `failed` + najwazniejsze sygnaly, pelne logi na zadanie.
- Process governance: `list`, `status`, `kill`, timeouty, limity rownoleglych procesow.
- Transparentna integracja shell (`ct-install`) dla `cd` i standardowego UX CLI.

### Differentiatory (przewaga nad alternatywami)

- Risk scoring komend (low/medium/high) i automatyczne wymuszanie `ask` dla operacji ryzykownych.
- Redakcja sekretow w outputach i logach (tokeny, klucze, hasla, dane wrazliwe).
- Profile bezpieczenstwa: `safe`, `balanced`, `power`.
- Reproducible run bundles: eksport metadanych decyzji policy + logow dla debug/audytu.
- Kompaktory per typ zadania (build/test/install/git) zamiast jednego generycznego skrotu.

### V2 (zwiekszenie adopcji)

- Policy as code: `.ct/policy.toml` + dziedziczenie ustawien globalnych `~/.ct/config.toml`.
- Tryb CI/non-interactive (bez promptow, twarde zasady).
- Hooki/pluginy (`pre-run`, `post-run`, `redact`, `summarize`).
- Budzet kontekstowy (`max output tokens`) i automatyczny fallback do `compact`.
- API wyszukiwania runow i logow po statusie, komendzie, czasie i poziomie ryzyka.

## Zasada redukcji outputu (AI context)

- Domyslnie dla AI zwracac wynik skrocony i strukturalny (status + najwazniejsze sygnaly).
- Pelny output pozostaje w logach i jest dostepny na zadanie.
- Dla zadan typu build/test preferowac wynik syntetyczny (np. `success`) zamiast pelnego strumienia, chyba ze AI jawnie poprosi o szczegoly.

## Progressive disclosure bez `run_id`

- Po wykonaniu `ct <cmd...>` narzedzie zwraca domyslnie tylko wynik syntetyczny (`SUCCESS` / `FAILED`).
- Szczegoly sa pobierane dopiero na zadanie z logow ostatniego wykonania.
- Domyslny kontekst zapytan to "ostatnie wykonanie" (bez podawania `run_id`).
- Brak dodatkowych plikow `json` z podsumowaniami; single source of truth to pliki logow w `~/.ct/logs`.

## Wskaznik ostatniego wykonania

- `ct` po kazdym wykonaniu aktualizuje wskaznik "latest" na ostatni plik logu.
- Implementacja wskaznika: symlink `~/.ct/logs/latest` -> `<najnowszy_plik>.log` (fallback: plik `.latest` z nazwa pliku).
- Komendy analityczne bez argumentow czytaja domyslnie log wskazany przez `latest`.
- Jezeli brak poprzedniego wykonania, komendy analityczne zwracaja czytelny blad.

## Komendy analityczne CLI (domyslnie na `latest`)

- `ct-ctl warnings` - pokazuje warningi z ostatniego wykonania.
- `ct-ctl errors` - pokazuje errory z ostatniego wykonania.
- `ct-ctl logs` - pokazuje pelny log ostatniego wykonania.
- `ct-ctl logs --filter warning|error` - filtrowanie logu po typie/sygnale.

## Plan rozszerzenia filtrow per narzedzie

- Aktualnie skonfigurowane: `filters.cargo`, `filters.maven`.
- Kolejne profile do dodania (w tym samym modelu `filters.<tool>.enabled`):
  - `filters.gradle` (`gradle`/`gradlew`)
  - `filters.npm`
  - `filters.pnpm`
  - `filters.yarn`
  - `filters.gcc`
  - `filters.gpp` (`g++`)
  - `filters.clang`
  - `filters.clangpp` (`clang++`)
  - `filters.make`
  - `filters.cmake` (build)
  - `filters.go` (`go build`/`go test`)
  - `filters.pytest`
  - `filters.dotnet` (`dotnet build`/`dotnet test`)
  - `filters.jest`
  - `filters.vitest`

## Extractory oparte tylko o log

- Wszystkie extractory dzialaja na tym samym parserze logu (`CMD`/`STDOUT`/`STDERR`/`EXIT`).
- Domyslny extractor: status operacji (`SUCCESS` / `FAILED`) na podstawie rekordu `EXIT`.
- Kolejne extractory (warning/error/highlights) sa uruchamiane na zadanie i parsuja ten sam log `latest`.
- Brak cache podsumowan na dysku; jednolity mechanizm dla wszystkich extractorow.

## Zasada CLI (transparentnosc)

- `ct <komenda> [args...]` ma dzialac jak bezposrednie uruchomienie `<komenda> [args...]`.
- Przyklad: `ct ls` ma zachowywac sie jak `ls`.
- `stdout` i `stderr` maja byc przekazywane 1:1 do terminala.
- Kod wyjscia procesu ma byc przekazywany dalej bez zmian.
- Bledy uruchomienia (np. brak komendy) maja byc zwracane transparentnie, bez "ukrywania".

## Tryby pracy CLI

- Tryb domyslny: transparentny wrapper polecenia (`ct <cmd> ...`).
- Tryb serwera MCP: osobny subcommand, np. `ct mcp` (transport `stdio`).
- Pozwala to miec jedno narzedzie do lokalnego uruchamiania i integracji MCP.

## Podzial binarek (decyzja)

- `ct` - tylko transparentne wykonywanie komend (`ct <cmd> [args...]`).
- `ct-ctl` - komendy kontrolne/analityczne (np. logi, warningi, errory, status, kill).
- Cel: unikniecie kolizji nazw (np. `ct logs` vs systemowa komenda `logs`) i zachowanie pelnej transparentnosci `ct`.

## Wstepna skladnia CLI

- `ct <cmd> [args...]`
- `ct -- <cmd> [args...]` (jawne oddzielenie argumentow `ct` od komendy)
- `ct mcp`
- `ct-install` (instalacja integracji shellowej do `bash`)
- `ct --help`

## Obsluga `cd` (integracja shellowa)

- `cd` ma byc wspierane przez `ct`, ale wymaga integracji z `bash`.
- `ct-install` dopisuje do `~/.bashrc` funkcje-wrapper `ct()`.
- Wrapper przechwytuje specjalna odpowiedz z binarki `ct` i wykonuje `eval` tylko dla zaufanego prefiksu (np. `__CT_BUILTIN__`).
- Dla `ct cd ...` faktyczna zmiana katalogu wykonuje sie w biezacej sesji shella.
- Dla zwyklych komend (`ct ls`) zachowanie zostaje transparentne (passthrough I/O + exit code).
- Wymagana transparentnosc dla uzytkownika: `ct cd ~`, `ct cd -`, `ct cd`, `ct cd ..` maja zachowywac sie jak natywne `cd`.
- Interpretacja argumentow `cd` (np. `~`, `-`, `CDPATH`) ma byc wykonywana przez shell, nie przez samą binarke `ct`.
- W v1 scope builtinow ograniczamy do `cd`; pozostale builtiny shella sa poza zakresem.

## `ct-install` - zakres

- Dodaje blok konfiguracyjny `# >>> ct >>> ... # <<< ct <<<` do `~/.bashrc` (idempotentnie, bez duplikacji).
- Nie nadpisuje recznej konfiguracji uzytkownika poza tym blokiem.
- Tworzy backup `~/.bashrc` przed pierwsza modyfikacja (np. `~/.bashrc.ct.bak`).
- Wypisuje instrukcje aktywacji: `source ~/.bashrc`.
- Opcjonalnie w przyszlosci: `ct-install --uninstall` usuwa blok `ct`.

## Format logow (`~/.ct/logs`)

- Jeden plik logu na jedno wykonanie komendy, np. `~/.ct/logs/<run_id>.log`.
- Kazda linia logu zaczyna sie timestampem w UTC (`RFC3339`, z ulamkiem sekundy).
- W logu musi byc zapisana uruchomiona komenda.
- Rekordy `CMD`, `STDOUT`, `STDERR` zapisywane sa jako `json_string`, aby bezpiecznie przenosic znaki nowej linii i inne znaki specjalne.
- Proponowany zapis linii:
  - `<timestamp> CMD <json_string>`
  - `<timestamp> STDOUT <json_string>`
  - `<timestamp> STDERR <json_string>`
  - `<timestamp> EXIT <exit_code>`

Przyklad:

```text
2026-04-13T10:15:22.184392Z CMD "printf \"a\\nb\\n\""
2026-04-13T10:15:22.184901Z STDOUT "a\nb\n"
2026-04-13T10:15:22.185044Z STDERR "ls: cannot access 'x': No such file or directory"
2026-04-13T10:15:22.185200Z EXIT 2
```

## Plan dalszego projektowania

1. Doprecyzowac kontrakt CLI transparentnego wrappera (I/O, exit code, sygnaly).
2. Zaprojektowac i zaimplementowac `ct-install` dla `bash`.
3. Ustalic format danych i uklad plikow w `~/.ct`.
4. Przygotowac szkielet projektu Cargo.
5. Zaimplementowac `ct <cmd> [args...]` z pelnym passthrough `stdout/stderr`.
6. Dodac obsluge `cd` przez shell-bridge.
7. Dodac zapis metadanych i logow do `~/.ct`.
8. Dodac tryb `ct mcp` (stdio).
9. Dodac testy i scenariusze weryfikacyjne.

## Uwagi

Szczegoly API MCP sa celowo odlozone - najpierw domykamy kontrakt CLI.
