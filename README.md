# SupportViewer client

Ứng dụng hỗ trợ từ xa nội bộ, dựa trên [RustDesk](https://github.com/rustdesk/rustdesk) `1.5.1`
(mã nguồn gốc giữ nguyên, xem `README-RustDesk.md`).

## Khác gì so với RustDesk

| Thay đổi | File |
|---|---|
| Public key kiểm tra chữ ký `custom.txt` là key của server SupportViewer thay cho key của RustDesk Ltd. Chỉ `custom.txt` do trang quản trị SupportViewer ký mới được nhận. | `src/common.rs` (`const KEY`) |
| Icon và logo SupportViewer | `res/`, `flutter/assets/`, `flutter/windows/runner/resources/`, `flutter/macos/Runner/AppIcon.icns`, `flutter/android/app/src/main/res/mipmap-*` |
| Workflow build bấm tay, chỉ các nền tảng cần dùng; thêm file zip thư mục app Windows / `RustDesk.app` macOS vào release | `.github/workflows/supportviewer-build.yml`, `.github/workflows/flutter-build.yml` |
| Các workflow tự chạy khi push / theo lịch của RustDesk được tắt | `.github/workflows-disabled/` |

## Không chứa cấu hình server

Bản build ở repo này là bản **chung**: không có địa chỉ server, key hay cài đặt nào.
Tên app, server và cài đặt nằm trong `custom.txt` do trang quản trị SupportViewer tạo, ký
và gói kèm khi tải về. Không có `custom.txt` hợp lệ thì app chạy như RustDesk bình thường.

## Build

GitHub → Actions → **SupportViewer build** → Run workflow. File build được đính vào release theo tag
nhập khi chạy (mặc định `sv-1.5.1`).

Secrets cần có trong repo (chỉ cho Android): `ANDROID_SIGNING_KEY` (PKCS12, base64),
`ANDROID_ALIAS`, `ANDROID_KEY_STORE_PASSWORD`, `ANDROID_KEY_PASSWORD`. Bản Windows / macOS không ký số
(Windows SmartScreen và macOS Gatekeeper sẽ hỏi khi mở lần đầu).

## Giấy phép

AGPL-3.0, như RustDesk (xem `LICENSE`). Không dùng tên và logo "RustDesk" cho bản phân phối này.
