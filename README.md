# 🚀 MinPanel (Lua Edition)

MinPanel là một công cụ quản trị hosting (control panel) **siêu nhẹ, di động và mạnh mẽ** dành cho Windows. Dự án được xây dựng trên nền tảng Rust hiệu năng cao kết hợp với hệ thống plugin linh hoạt bằng Lua, mang lại trải nghiệm quản lý server tối ưu.

---

## ✨ Tính năng nổi bật

- 📊 **Dashboard trực quan**: Giám sát tài nguyên hệ thống (CPU, RAM, Disk, Network) theo thời gian thực với các biểu đồ sống động.
- 🌐 **Quản lý Website**: Khởi tạo và cấu hình website chỉ trong vài giây. Hỗ trợ tự động hóa SSL, tùy chỉnh phiên bản PHP và quản lý Virtual Hosts.
- 📦 **Cửa hàng ứng dụng (App Store)**: Cài đặt, gỡ bỏ và quản lý vòng đời các dịch vụ máy chủ (Apache, PHP, MySQL, Redis...) thông qua các plugin Lua script.
- 🖱️ **Giao diện Windows Native**: Tích hợp Tray icon tiện lợi, cho phép khởi động nhanh và truy cập bảng điều khiển tức thì.
- ⚡ **Siêu nhẹ & Portable**: Chạy trực tiếp dưới dạng ứng dụng standalone, không cần cài đặt rườm rà hay để lại rác trong hệ thống.

## 📸 Ảnh chụp màn hình

### Dashboard
![Dashboard](./assets/screenshots/dashboard.png)

---

## 🚀 Hướng dẫn nhanh

### 1. Build từ mã nguồn
Nếu bạn muốn tự tay xây dựng ứng dụng:
1. Clone repository:
   ```bash
   git clone https://github.com/hocdev-com/MinPanel.git
   cd MinPanel
   ```
2. Build ứng dụng:
   ```bash
   cargo build --release
   ```
   *File thực thi sẽ nằm tại: `target/release/MinPanel.exe`*

### 2. Đăng nhập lần đầu
Sau khi khởi động ứng dụng, bạn có thể truy cập giao diện quản trị qua trình duyệt (mặc định tại `http://localhost:8080`).

> [!IMPORTANT]  
> **Thông tin đăng nhập mặc định:**
> - **Tài khoản:** `admin`
> - **Mật khẩu:** `admin`

---

## 🛠️ Công nghệ cốt lõi

- **Core Engine**: [Rust](https://www.rust-lang.org/) (Axum, Tokio) - Đảm bảo tốc độ và an toàn bộ nhớ.
- **Frontend**: HTML5, Vanilla CSS, Javascript (Modern design, No frameworks).
- **Extensibility**: [Lua](https://www.lua.org/) - Cho phép cộng đồng dễ dàng phát triển plugin mới.
- **Desktop Integration**: Win32 API (windows-sys) - Tối ưu hóa cho hệ điều hành Windows.

## 📂 Cấu trúc thư mục

- `src/`: Mã nguồn Rust xử lý logic backend và GUI.
- `src/ui/`: Chứa các giao diện Dashboard (HTML/CSS/JS).
- `data/plugins/`: Hệ thống plugin Lua điều khiển dịch vụ (Apache, PHP, etc.).
- `assets/`: Tài nguyên hình ảnh, icon và tài liệu hướng dẫn.

## 🤝 Đóng góp & Hỗ trợ

Chúng tôi luôn hoan nghênh mọi ý tưởng đóng góp và báo lỗi.
- 🐛 **Báo lỗi**: Vui lòng tạo [Issue](https://github.com/hocdev-com/MinPanel/issues).
- 💡 **Đóng góp**: Gửi Pull Request với các tính năng mới hoặc plugin hữu ích.

---
*Phát triển bởi đội ngũ **MinPanel Team**.*
