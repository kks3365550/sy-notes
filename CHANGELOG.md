# 변경 이력

버전은 `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`을 같이 올리고 git 태그 `vX.Y.Z`를 단다.

## 1.1.0 - 2026-11-23
### 추가
- 앱 시작 시 하루 1회 자동 DB 백업(`data/backups/`, 일 7개·주 4개 회전)
- 사이드바 하단 '백업'·'내보내기' 버튼, ZIP 내보내기 기본 경로 `data/exports/`
### 수정
- `App.css` 미적용으로 UI가 스타일 없이 보이던 문제
- 첨부 이미지 `attachment://` 제공, AI 응답 30초 절단, 자동저장 낙관적 락 실패, 휴지통 정리·고아 첨부 삭제 (v1.0.0 이후 누적)
### 알려진 한계
- 사이드바 검색이 백엔드 FTS5를 쓰지 않음(목록 내 클라이언트 검색)
- WebView 캐시 `data/` 격리 코드 없음
- 50,000자 IPC p95 재측정 필요

## 1.0.0
- Notion-Lite 최초 릴리즈 (태그 `v1.0.0-release`)
