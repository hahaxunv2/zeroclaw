# Bản đồ CI/CD ZeroClaw

Tài liệu này ánh xạ các đường dẫn trong kho lưu trữ với các workflow GitHub Actions quản lý chúng.

## Các Workflow chính

- `.github/workflows/ci.yml` (`CI`)
    - Mục đích: Kiểm tra Rust (`test`, `build`) trên các PR hướng tới `master`.
    - Hành vi: Sử dụng `cargo nextest` để kiểm tra và xây dựng các bản release cho Linux và macOS.
    - Chặn merge: Cần thiết cho tất cả các PR vào `master`.

- `.github/workflows/release-build.yml` (`Production Release Build\?)
    - Mục đích: Xây dựng các bản binary sản xuất có tính tái lập trên các lượt push vào `master` và các tag `v*`.
    - Kiểm tra chất lượng: `fmt`, `clippy`, và `test` trước khi build.

- `.github/workflows/release.yml` (`Release\?)
    - Mục đích: Xử lý việc tạo GitHub release và xuất bản artifact.

## Các Workflow quan trọng

- `.github/workflows/pr-check-stale.yml` (`Stale\?)
    - Mục đích: Tự động hóa vòng đời issue/PR bị cũ (stale).
- `.github/dependabot.yml` (`Dependabot\?)
    - Mục đích: Các PR cập nhật thư viện được nhóm và giới hạn tần suất (Cargo + GitHub Actions).
- `.github/workflows/pr-check-status.yml` (`PR Hygiene\?)
    - Mục đích: Nhắc nhở các PR cũ nhưng vẫn đang hoạt động cần rebase hoặc chạy lại các kiểm tra bắt buộc.

## Bản đồ Trigger

- `CI`: Các Pull Request hướng tới `master`.
- `Production Release Build`: Push vào `master`, Push các tag `v*`.
- `Release`: Push tag `v*`, Dispatch thủ công.
- `Security Audit`: Push vào `master`, PR vào `master`, lịch trình hàng tuần.

## Hướng dẫn Triage nhanh

1. `CI` thất bại: Kiểm tra `.github/workflows/ci.yml`.
2. Lỗi build release: Kiểm tra `.github/workflows/release-build.yml`.
3. Lỗi release: Kiểm tra `.github/workflows/release.yml`.
