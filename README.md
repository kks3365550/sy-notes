# sy-notes 📝

> **초경량 로컬 전용 개인 지식 및 업무일지 관리 애플리케이션 (Notion-Lite in Rust & Tauri v2)**

`sy-notes`는 노션(Notion)의 검증된 사용자 경험(블록 기반 리치 에디터, 구조화된 일지 템플릿, 로컬 SQLite WAL 캐싱, AI 연동)을 계승하면서, 무거운 Electron 런타임, 비싼 클라우드 구독료, 한국어 IME 자소 분리 버그를 해결한 100% 로컬 지향 데스크탑 앱입니다.

---

## ⚡ 핵심 기능 (Key Features)

- **블록 기반 리치 에디터**: BlockNote Core (MPL-2.0) 기반의 위지윅(WYSIWYG) 블록 에디터 (`/` 슬래시 메뉴, 체크리스트, 헤딩, 불릿, 인용구 등 지원).
- **한국어 IME 조합 완벽 방어**: `compositionstart` 시 자동저장 일시 보류, `compositionend` 후 500ms 디바운스 flush로 타이핑 중 받침 쪼개짐/글자 유실 0건 보장.
- **원터치 당일 업무일지**: 사이드바의 `[오늘 업무일지]` 버튼 클릭 시 오늘 날짜의 업무일지(목표, 진행 업무, 특이사항)를 원자적 트랜잭션으로 생성 및 즉시 로드.
- **SQLite FTS5 + trigram 한국어 전문 검색**: 3글자 이상 키워드는 FTS5 trigram 색인 MATCH 검색, 1~2글자는 LIKE 폴백으로 실시간 고속 검색. 휴지통 문서는 검색 결과에서 100% 격리.
- **저장 충돌 처리**: 낙관적 락(`updated_at`)으로 다른 창의 저장을 감지하면 **내 내용으로 덮어쓰기 / 저장된 내용 불러오기**를 선택. 저장 실패는 숨기지 않고 배너로 표시하며, 변경 내용은 저장될 때까지 보관.
- **LLM 스트리밍 (AI 패널)**: `Ctrl + J` 단축키로 토글되는 AI 패널에서 실시간 3줄 요약 및 문맥 질의 스트리밍. 기본값은 로컬 Ollama (`http://127.0.0.1:11434/v1`)이며, 패널의 ⚙ 설정에서 다른 OpenAI 호환 서버 주소와 모델을 지정할 수 있습니다(재시작 후에도 유지). 로컬이 아닌 주소를 지정하면 문서 내용이 그 서버로 전송됩니다.
- **이미지 첨부**: 붙여넣은 이미지는 `data/attachments/`에 저장되고 전용 `attachment://` 스킴으로 표시됩니다.
- **포터블 모드 & 백업 복구**: 실행 파일(`sy-notes.exe`) 옆 `data/` 디렉터리에 SQLite DB, 첨부파일, WebView 캐시를 격리 저장. 7일 일간 + 4주 주간 자동 회전 백업 및 ZIP 내보내기 지원. 휴지통은 30일이 지나면 자동 정리되고, 어떤 문서도 참조하지 않는 첨부파일도 함께 삭제됩니다.
- **읽기 전용 매체 안전 폴백**: USB 등 쓰기 불가 매체에서 실행 시 `%APPDATA%/sy-notes/data`로 자동 안전 폴백 및 UI 경고 배지 표시.

---

## 📊 실측 성능 지표 (Performance Benchmarks)

별도 표기가 없으면 Windows 실측값입니다.

| 지표 | 측정 결과 | 목표 기준치 | 상태 |
|---|---|---|:---:|
| **8스레드 동시 읽기/쓰기 10,000회** | `database is locked` **0건** (3,844 ops/sec, p95 5.82ms) | 0건 락 에러 | **PASS** |
| **50,000자 텍스트 IPC 왕복 지연** | **재측정 필요** (아래 참고) · 백엔드 구간 p95 **4.46 ms** (JSON 0.38 ms + DB upsert/FTS 4.14 ms, Linux Release) | < 16.0 ms | 재측정 |
| **프로세스 트리 WorkingSet 메모리** | 합산 **2.66 MB ~ 9.59 MB** (Private 합산 122.5 MB) | < 150 MB | **PASS** |
| **콜드 스타트 창 표시 시간** | 평균 **143 ms** (0.143초) | < 1,000 ms | **PASS** |
| **실행 파일 크기** | **8.78 MB** (단독 실행 파일) | 노션(118.8MB)의 1/13 | **PASS** |
| **테스트 스위트 통과율** | Rust **64건** (단위 56 + 통합 8), 프론트엔드 **16건** 전부 통과 | 0 실패 | **PASS** |

> **IPC 수치 재측정 필요:** 이전에 기록된 p95 6.74 ms는 `benchmarks/measure_ipc_cdp.mjs`로 측정한 값인데, 당시 스크립트는 같은 문서를 반복 저장하면서 잘못된 버전을 보내 10번째 반복부터 저장이 실패하는 구조였습니다. 스크립트는 수정됐으므로 Windows에서 `npm run tauri dev` 실행 후 CDP 포트를 열고 다시 측정해 이 표를 갱신하세요. WebView/IPC 전송 비용은 그 결과에서 백엔드 구간 p95를 뺀 값입니다.

---

## 🛠 기술 스택 (Tech Stack)

- **Backend**: Rust 2024, Tauri v2, `rusqlite 0.42` (bundled, WAL mode), `r2d2 0.8`, `tokio`, `reqwest`
- **Frontend**: React 19, TypeScript, Vite, Tailwind CSS v4, `@blocknote/core` (MPL-2.0), `@blocknote/react`
- **Database**: SQLite 3 (WAL journal mode, busy_timeout 5000ms, synchronous NORMAL, FTS5 trigram tokenizer)

---

## 🚀 시작하기 (Getting Started)

### 사전 요구 사항
- [Node.js](https://nodejs.org/) v20 이상
- [Rust](https://www.rust-lang.org/) 1.85 이상

### 개발 서버 실행
```powershell
# 의존성 설치
npm install

# 프론트엔드 및 Tauri 개발 창 실행
npm run tauri dev
```

### 프로덕션 빌드
```powershell
# 프론트엔드 빌드 및 릴리즈 실행 파일 생성
npm run tauri build
```
빌드된 단독 실행 파일은 `src-tauri/target/release/sy-notes.exe`에 생성됩니다.

### 테스트 실행
```powershell
cd src-tauri

# 전체 (단위 56 + 통합 8)
cargo test

# 백엔드 단위/카오스/보안 테스트만
cargo test --lib

# 50,000자 왕복 p95와 단계별 분해 (JSON 직렬화 / DB upsert+FTS)
cargo test --release --lib test_50000_char_ipc -- --nocapture

# 동시성 10,000회 락 프리 벤치마크
cargo test --test concurrency_test -- --nocapture

# 한국어 3자 FTS5 및 1~2자 LIKE 검색 검증
cargo test --test fts_test
```

```powershell
# 프론트엔드 16건: IME 조합 중 저장 보류 로직, 첨부 URL 변환 (vitest + jsdom)
npm test
```
실제 한글 입력 확인은 `tests/MANUAL_IME_CHECK.md` 절차로 사람이 합니다.

PR과 master 푸시마다 GitHub Actions(`.github/workflows/ci.yml`)가 프론트엔드 검사(tsc, `npm test`, vite build)와 Rust 테스트(Linux, Windows/MSVC)를 자동으로 실행합니다.

---

## 📜 라이선스 (License)

- BlockNote Core: MPL-2.0
- Application Code: MIT License
