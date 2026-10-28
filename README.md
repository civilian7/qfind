# qfind

Rust로 만든 콘솔 파일 내용 검색기입니다. AI 에이전트가 호출하기 쉽도록 **기본 리터럴 검색**, **NDJSON**, **전체 결과 제한**, **명확한 종료 코드**를 제공합니다.

`qfind`는 독립 실행 파일이며 `find`, `findstr`, `rg`를 내부에서 호출하지 않습니다. 파일 탐색은 `ignore`, 기본 문자열 검색은 `memchr::memmem`의 CPU별 최적화 구현, 정규식은 Rust `regex`를 사용합니다. 여러 파일을 병렬로 탐색·검색하고 출력은 버퍼링합니다.

## 빌드와 실행

Rust 1.88 이상이 필요합니다.

```powershell
cargo build --release --locked
.\target\release\qfind.exe --help
.\target\release\qfind.exe '검색할 문자열' .
```

성능 측정과 실사용에는 `target/release` 실행 파일을 사용하세요. `cargo run`은 빌드 확인 시간이 추가되고 기본적으로 최적화되지 않은 디버그 실행 파일을 사용합니다.

Windows에서는 `qfind.exe`를 PATH에 등록된 폴더에 복사하면 다른 프로젝트에서도 호출할 수 있습니다. Linux/macOS에서는 `target/release/qfind`를 사용합니다.

## 검색 예시

```powershell
# 기본: 하위 폴더까지 검색. a.b는 정규식이 아닌 문자열 그대로 검색
qfind 'a.b' .

# 여러 파일 또는 폴더
qfind 'TODO' src tests README.md

# 한글 및 대소문자 구분 없는 검색
qfind '오류 메시지' src
qfind -i 'error' src

# 정규식은 명시적으로 활성화
qfind -r 'fn\s+\w+' src

# 여러 검색어를 OR로 검색. -e를 사용하면 위치 인수는 모두 경로
qfind -e 'load_config' -e 'save_config' src tests

# 단어 전체 / 줄 전체 일치
qfind -w 'config' src
qfind -x 'enabled=true' .

# 전후 3줄 문맥, 또는 앞 2줄·뒤 5줄
qfind -C 3 'load_config' src
qfind -B 2 -A 5 'load_config' src

# 경로 순서를 정한 뒤 검색: 결과 제한을 적용해도 선택 결과가 일정함
qfind --json --sort path --max-results 20 'load_config' src

# 파일 종류 제한 및 제외. 패턴은 셸에서 확장되지 않도록 따옴표 사용
qfind -g '*.rs' -g '!generated/**' 'TODO' .

# 파일 경로만 / 파일별 일치 줄 수 / 존재 여부
qfind -l 'TODO' .
qfind -c 'TODO' .
qfind -q 'TODO' .

# 파일 내용은 읽지 않고 검색 대상 경로만 나열
qfind --files -g '*.rs' src

# 숨김·무시 파일까지 검색
qfind --all 'TODO' .

# 스레드 수 및 진단 통계 (통계는 stderr)
qfind -j 8 --stats 'TODO' .

# 하이픈으로 시작하는 검색어
qfind -- '--flag' .
```

기본 출력은 `경로:줄:열:내용`이며 줄·열은 1부터 시작합니다. 열은 **UTF-8 바이트 위치**입니다. 한 줄에 여러 번 일치해도 한 번만 출력하며 첫 일치의 열을 표시합니다. `-c`는 출현 횟수가 아닌 일치한 줄 수입니다. `-e`는 기본적으로 각 검색어를 리터럴로 해석하고 `-r`과 함께 쓰면 정규식으로 해석합니다. 가장 앞에서 시작하는 일치를 선택하며, 시작 위치가 같으면 검색어 순서에 따릅니다. `-w`는 Unicode 단어 구성 문자(`\w`)를 기준으로 앞뒤 경계를 검사하고, `-x`는 줄 전체를 검사합니다. 함께 쓰면 `-x`를 우선합니다.

문맥은 `경로-줄-내용`으로 출력합니다. `-C`는 전후 공통값이며 `-A`·`-B`로 각각 덮어쓸 수 있습니다. 겹치는 문맥은 파일 내에서 한 번만 출력하고 일치 줄은 문맥 대신 검색 결과로 출력합니다. 문맥 옵션은 `-c`·`-l`·`-q`·`--files`와 함께 사용할 수 없습니다.

## AI 에이전트에서 호출

```powershell
qfind --json -C 2 --max-results 100 --max-output-bytes 65536 --timeout-ms 5000 --max-file-bytes 16777216 --max-line-buffer-bytes 1048576 '검색할 문자열' 'E:\works\projects\example'
```

```json
{"type":"match","path":"src/main.rs","line":12,"column":5,"text":"    // TODO: implement"}
{"type":"summary","schema_version":1,"records_emitted":1,"matched":true,"scan_complete":true,"stop_reason":"completed","error_count":0}
```

stdout의 각 줄을 독립적인 JSON 객체로 해석하고 `type`으로 구분하세요. **JSON 모드는 마지막에 `summary`를 한 개 출력**합니다. 결과가 없거나 오류가 나도 요약을 출력합니다. JSON 경로는 기본적으로 현재 작업 폴더 기준 상대 경로이며 구분자는 `/`입니다. 오류와 `--stats` 결과는 stderr에 NDJSON으로 출력합니다. 색상 코드나 진행 메시지는 stdout에 섞이지 않습니다. `--help`와 `--version`은 일반 안내문을 출력하고 검색 요약을 생성하지 않습니다.

| 모드 | JSON 필드 |
| --- | --- |
| 기본 검색 | `type: "match"`, `path`, `line`, `column`, `end_column`, `byte_start`, `byte_end`, `text` |
| 문맥 | `type: "context"`, `path`, `line`, `byte_start`, `byte_end`, `text` |
| `-l`, `--files` | `type: "file"`, `path` |
| `-c` | `type: "count"`, `path`, `count` |
| 종료 요약 | `type: "summary"`, `schema_version`, `records_emitted`, `matched`, `scan_complete`, `stop_reason`, `error_count` |

`--max-results N`은 **모든 파일을 합쳐 최대 N개 출력 레코드**를 허용합니다. 기본 모드는 일치 줄, `-l`·`-c`·`--files`에서는 파일 단위입니다. 제한에 도달하면 검색을 조기 종료하므로 전체 결과 수를 의미하지 않습니다. 병렬 실행 중에도 상한을 초과하지 않습니다. 속도를 위해 파일 간 출력 순서는 보장하지 않습니다.

`records_emitted`는 `summary`를 제외한 모든 stdout 레코드 수이며 **문맥을 포함**합니다. `context_records_emitted`는 그중 문맥 레코드 수입니다. `--max-results`는 문맥을 세지 않고 일치 줄 또는 파일 단위 결과만 제한합니다. 제한 도달 시 뒤 문맥을 포함해 즉시 중단하므로 마지막 일치의 문맥이 덜 나올 수 있습니다. 문맥도 출력 바이트 예산을 사용합니다.

`matched`는 결과를 발견했는지 표시하며, 출력 예산 때문에 일치 레코드가 0개여도 `true`일 수 있습니다. `scan_complete`는 지정된 검색 범위와 모드의 처리를 오류나 자원 제한에 의한 생략·중단 없이 완료했는지 나타냅니다. 기본 제외 규칙에 해당하는 파일까지 검색했다는 의미는 아닙니다. `-l`은 파일별 첫 일치에서 읽기를 끝내도 해당 모드의 정상 처리입니다.

`column`과 `end_column`은 원래 줄에서 **1부터 시작하는 UTF-8 바이트 위치**이며 끝 위치는 포함하지 않습니다. `byte_start`와 `byte_end`는 **원래 파일에서 0부터 시작하는 반열린 바이트 범위**입니다. 문맥의 범위는 해당 줄 전체이며 줄 끝 LF·CRLF를 제외합니다. 검색 결과의 범위는 첫 일치만 나타냅니다. 발췌 여부에 관계없이 원래 파일 기준을 유지합니다. 파일 변경을 감지하거나 편집을 검증하는 정보는 아니므로, 후속 수정 전에는 파일이 여전히 같은 내용인지 확인하세요.

| `stop_reason` | 의미 |
| --- | --- |
| `completed` | 요청한 검색을 완료함. 일치가 없는 경우도 포함 |
| `result_limit` | 결과 개수 제한에 도달해 중단 |
| `output_limit` | 다음 결과 레코드가 출력 바이트 예산을 초과해 중단 |
| `error` | 인수·검색어·glob 설정, 파일 읽기 또는 탐색 오류 발생 |
| `time_limit` | 시간 제한에 도달해 중단 |
| `resource_limit` | 파일·줄 크기 상한으로 일부 내용을 건너뜀 |

제한에 정확히 N개 도달해도 `scan_complete: false`입니다. 나머지 범위를 검색하지 않았으므로 추가 결과가 있다는 뜻으로 해석하면 안 됩니다. 제한 도달과 오류가 함께 발생하면 `stop_reason`과 `error_count`를 함께 확인하세요. stdout 파이프가 닫히거나 쓰기가 실패하면 마지막 요약이 없을 수 있습니다. 외부 강제 종료를 포함해 요약이 없는 출력은 완료된 응답으로 취급하지 마세요.

### 출력 예산과 긴 줄

- `--max-output-bytes N`: JSON stdout의 **직렬화 후 바이트 수** 상한입니다. 최소 1024이며 기본값은 무제한입니다. 경로·JSON 이스케이프·base64·개행·마지막 요약을 포함합니다. stderr는 포함하지 않습니다. 요약용으로 768바이트를 미리 남기므로 실제 출력은 상한보다 적을 수 있습니다. 병렬 실행에서도 상한을 지키며 JSON 객체를 중간에서 자르지 않습니다.
- `--max-line-bytes N`: JSON의 각 `text`를 원본 기준 최대 N바이트로 발췌합니다. 기본값은 **4096**, `0`은 무제한입니다. 첫 일치 위치 주변을 발췌하며 UTF-8이 유효한 줄은 문자 경계를 지킵니다. 아주 작은 제한에서는 문자가 들어가지 않아 빈 발췌가 생길 수 있습니다. JSON 이스케이프 때문에 직렬화 크기는 N바이트보다 클 수 있습니다.
- 두 옵션은 `--json`과 함께 사용합니다. 일반 텍스트 출력에는 기본 발췌 제한을 적용하지 않습니다.

줄이 발췌되면 다음 메타데이터가 추가됩니다. `column`은 **원래 줄의 1부터 시작하는 바이트 열**을 유지하고, `text_start_byte`는 원래 줄에서 발췌 시작 위치를 **0부터** 셉니다. `text_original_bytes`는 줄 끝 개행을 제거한 원래 줄의 바이트 수입니다.

```json
{"type":"match","path":"large.json","line":1,"column":10001,"text":"...needle...","text_truncated":true,"text_start_byte":9997,"text_original_bytes":500000}
```

발췌는 출력 내용만 줄이며 검색 대상이나 검색 결과 개수를 줄이지 않습니다. 읽기 메모리 상한을 설정하는 기능도 아닙니다. **남은 출력 예산에 들어가지 않으면 발췌를 자동으로 더 줄입니다.** 이스케이프와 base64를 적용한 실제 레코드 크기를 기준으로 조정합니다. 긴 검색어 전체가 발췌 범위에 들어가지 않거나 내용이 빈 문자열이 될 수 있지만 원래 일치 범위는 유지합니다. 내용이 비어도 메타데이터가 예산에 들어가지 않으면 레코드를 출력하지 않고 `output_limit`으로 종료합니다.

### 읽기 자원과 검색 범위 통계

| 옵션 | 동작 |
| --- | --- |
| `--timeout-ms N` | 검색어 준비·탐색·읽기·검색 중 경계마다 경과 시간을 확인해 중단. 기본 무제한 |
| `--max-file-bytes N` | 상한보다 큰 파일을 건너뜀. 읽는 도중 파일이 커지는 경우도 탐지. 기본 무제한 |
| `--max-line-buffer-bytes N` | 개행을 제거한 원본 줄이 N바이트를 넘으면 그 줄을 건너뛰고 다음 줄부터 계속 검색. 기본 무제한 |

세 옵션의 N은 1 이상입니다. 파일·줄 크기 옵션은 `--files`와 함께 쓰지 않습니다. 파일 크기 검사가 메타데이터와 읽기 사이의 변경을 완전히 막지는 않으며, 증가 여부를 확인하기 위해 파일 상한보다 1바이트 더 읽을 수 있습니다. 시간 제한은 협력적 중단이므로 블로킹 OS I/O, 단일 정규식 연산, 경로 정렬 작업을 즉시 중단하는 엄격한 실행 시간 보장은 아닙니다.

시간 초과와 자원 상한에 의한 생략은 **종료 코드 2**, `scan_complete: false`로 보고합니다. 큰 파일·줄은 오류 레코드를 생성하지 않고 요약 카운터에 기록합니다. 줄을 건너뛰면 문맥을 끊고, `-c`에서는 그 파일의 부분 집계를 출력하지 않습니다. `-l`은 실제 일치를 발견한 파일을 정상적으로 출력할 수 있습니다.

요약에는 `files_visited`(검색 대상으로 방문한 파일), `files_searched`(바이너리 검사 통과 후 읽기 검색에 진입한 파일), `binary_files_skipped`, `oversized_files_skipped`, `lines_skipped`가 포함됩니다. 바이너리 기본 제외는 의도된 범위이므로 단독으로 검색을 불완전하게 만들지 않습니다. 읽는 중 커진 파일은 검색·크기 생략 양쪽 카운터에 포함될 수 있습니다. 숨김·무시 규칙으로 탐색하지 않은 파일의 전체 개수는 세지 않습니다.

### 순서와 경로

`--sort path`는 대상 경로를 먼저 수집하고 현재 작업 폴더 기준 `/` 경로의 문자열 순서로 정렬한 뒤 순차 검색합니다. 결과 제한은 그 후 적용합니다. 이 모드의 파일 검색은 `-j`와 관계없이 순차 처리하며 경로 목록 메모리와 최초 결과 지연이 늘어납니다. 파일 내용·검색 범위가 동일할 때 선택 결과가 재현됩니다. 옵션을 생략하면 기존 병렬 검색을 사용합니다.

`--path-style relative|absolute|native`로 결과와 구조화된 오류의 경로 표현을 선택합니다. 기본값은 JSON=`relative`, 일반 텍스트=`native`입니다. 상대 경로의 기준은 현재 작업 폴더이며 `relative`와 `absolute`는 `/` 구분자를 사용합니다. Windows에서 다른 드라이브 경로는 상대화할 수 없어 절대 경로로 표시합니다. 심볼릭 링크를 다른 경로로 바꾸는 정규화는 하지 않습니다.

### 구조화된 오류

JSON 모드에서 인수 검증·정규식·glob·탐색·읽기 오류는 stderr에 다음 형태로 출력합니다. JSON 파싱 실패를 피하려면 stdout과 stderr를 별도로 읽으세요.

```json
{"type":"error","code":"traversal_error","stage":"traversal","path":"missing.txt","message":"...","os_error":2}
```

`code`는 `invalid_arguments`, `invalid_pattern`, `invalid_glob`, `current_directory_error`, `traversal_error`, `read_error`, `output_error` 중 하나입니다. `stage`는 `arguments`, `pattern`, `glob`, `setup`, `traversal`, `read`, `output`입니다. `path`와 `os_error`는 확인 가능한 경우에만 제공합니다. 자동 분기는 `code`·`stage`를 사용하고 OS별로 달라지는 `message`를 파싱하지 마세요. `--stats`는 stderr에 `type: "stats"`인 JSON 레코드를 추가합니다.

오류 출력은 기본 **20개·16 KiB**로 제한합니다. `--max-errors N`과 `--max-error-bytes N`으로 각각 변경하며 `0`은 모두 생략합니다. 두 제한 중 먼저 닿는 것을 적용하고, 마지막 오류가 남은 바이트에 들어가지 않으면 메시지·경로를 줄여 완전한 레코드로 출력할 수 있습니다(`message_truncated: true`). 요약의 `error_count`는 관측한 전체 오류 수이고 `errors_emitted`·`errors_suppressed`는 출력·생략 수입니다. 생략해도 종료 코드는 2입니다. `--stats`의 통계 레코드는 오류 바이트 제한에서 제외됩니다. 인수 파싱 자체가 실패하면 오류 제한은 기본값을 적용합니다.

| 종료 코드 | 의미 |
| --- | --- |
| `0` | 일치 결과 있음 (`--files`는 경로 있음). 출력 파이프가 닫혀 정상적으로 조기 종료한 경우도 포함 |
| `1` | 일치 결과 없음 |
| `2` | 잘못된 인수·정규식, 파일 읽기·탐색·출력 오류 또는 시간·읽기 자원 상한에 의한 불완전 검색. 일부 결과가 이미 출력되었을 수 있음 |

읽기 오류가 하나라도 관측되면 일치 결과가 있어도 `2`입니다. `-q`나 결과 제한으로 조기 종료하면 아직 방문하지 않은 파일의 오류까지 검사하지는 않습니다.

## 기본 검색 범위

- `.gitignore`, `.ignore`, `.qfindignore`와 Git 전역 무시 규칙을 적용합니다. Git 무시 파일은 `ignore` 라이브러리의 Git 저장소 판정 규칙을 따릅니다.
- 숨김 파일과 폴더를 제외합니다. `--hidden`으로 포함합니다.
- `.git`, `node_modules`, `target`, `dist`, `build`, `.venv`, `venv`, `__pycache__`, `.next`, `.cache` 폴더를 제외합니다.
- `--no-ignore` (`-u`)는 무시 파일과 위 폴더 제외를 해제합니다. `--all`은 여기에 숨김 파일도 포함합니다.
- 명시한 파일·검색 시작 폴더는 기본 제외 규칙을 우회합니다. 그 아래 항목에는 규칙을 적용합니다. 제외된 하위 폴더는 `-g`만으로 다시 포함할 수 없으므로 `--no-ignore`를 함께 사용하세요.
- 심볼릭 링크는 기본적으로 따라가지 않습니다. `-L`로 활성화하며 순환은 오류로 보고합니다.
- stdout이 검색 폴더 내부의 파일로 리다이렉션되면 그 파일은 검색 대상에서 제외합니다.

## 문자와 큰 파일 처리

기본 검색은 파일의 바이트에서 UTF-8 검색어를 찾습니다. UTF-8 한글 파일을 지원하며 UTF-16이나 CP949를 자동 변환하지 않습니다. `-i`는 Unicode 대소문자 규칙을 사용하는 정규식 엔진으로 처리합니다.

LF·CRLF와 마지막 개행이 없는 파일을 처리합니다. 검색과 출력에서는 줄 끝 개행과 CRLF의 CR을 제거하며, 줄 내부와 파일 끝의 단독 CR은 보존합니다. 정규식은 개행을 제거한 **각 줄**에 적용하며 줄을 가로지르는 검색은 지원하지 않습니다.

첫 읽기의 최대 8 KiB에서 NUL 바이트가 발견된 파일은 바이너리로 간주해 건너뜁니다. 파일 전체의 바이너리 여부를 보증하는 검사는 아닙니다. `--text` (`-a`)로 이 검사를 해제할 수 있습니다. UTF-8이 아닌 출력 내용의 JSON `text`에는 대체 문자가 들어가며, 그 출력 내용의 원본 바이트를 복원할 수 있는 `text_base64` 필드가 추가됩니다. 발췌된 경우 base64도 발췌 구간만 포함합니다. 경로는 JSON에서 Unicode 문자열로 출력하며 표현할 수 없는 문자는 대체합니다.

일반적인 읽기 버퍼는 작업자당 256 KiB이고 출력은 작업자당 약 64 KiB씩 묶습니다. 읽기 버퍼를 재사용하며 파일 전체를 미리 적재하지 않습니다. 줄 상한을 생략하면 **긴 한 줄은 완성될 때까지 버퍼를 확장**하므로 메모리는 스레드 수와 각 작업자가 만난 가장 긴 줄에 따라 늘어납니다. 문맥 사용 시 작업자마다 앞 문맥 줄도 보관합니다. `--max-line-buffer-bytes`는 긴 줄 축적을 제한하지만 고정 읽기 버퍼, 메모리 할당 여유 공간, 앞 문맥, 정렬 경로 목록을 포함한 전체 프로세스 메모리 상한은 아닙니다. 파일 단위 병렬화이므로 대형 파일 하나만 검색할 때 여러 코어로 나누어 처리하지는 않습니다.

## 검증과 벤치마크

```powershell
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all --check
python scripts/benchmark.py
```

벤치마크는 기본적으로 1,028개 파일, 총 128 MiB의 ASCII 데이터를 `bench-data`에 만듭니다. 사용 가능한 `rg`, Windows `findstr`를 함께 측정합니다. 검색 결과 줄 수가 같은지 먼저 검사하고, 출력은 null 장치로 보내며, 실행 순서를 섞어 5회 측정한 중앙값을 기록합니다. 시간에는 프로세스 시작·탐색·읽기·검색·출력이 포함됩니다. 결과는 `bench-results.json`에 저장합니다.

반복 측정은 **캐시가 데워진 환경**의 결과입니다. 디스크·파일 구성·검색어·백신·출력량에 따라 성능은 달라집니다. `rg`보다 항상 빠르다고 가정하지 않습니다. 실제 측정 결과와 재현 조건은 [BENCHMARK.md](BENCHMARK.md)에 기록합니다.
