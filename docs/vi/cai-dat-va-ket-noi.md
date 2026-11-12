# Cài MagicPic và nối với Claude hoặc Codex

Trang này trả lời hai câu hỏi: nên **cài MagicPic về máy** hay **dùng phiên Claude Code trên
cloud**, và cài thế nào. Cách dùng hằng ngày, lời nhắn mẫu và giới hạn đã biết nằm ở
[`art-of-seeing-va-mcp.md`](art-of-seeing-va-mcp.md).

## 1. Nên chọn cách nào

**Để chỉnh ảnh: cài về máy.** Phiên trên cloud (như phiên đã phát triển MagicPic) hợp để
**sửa mã nguồn của MagicPic**, không hợp để chỉnh ảnh hằng ngày.

| | Cài về máy, nối Claude/Codex trên máy | Phiên Claude Code trên cloud |
|---|---|---|
| Chuẩn bị | Build một lần: đo được 10,5 phút và 1,6 GB thư mục build trên máy Linux 4 nhân; máy yếu hơn hoặc Windows có thể 20–40 phút | Mỗi phiên mới là một máy mới: tải mã nguồn và build lại (đo được khoảng 6 phút trên máy 4 nhân, chưa kể cài thư viện) |
| Ảnh của bạn | Nằm yên trên máy bạn | Phải tải lên máy cloud, rồi tải kết quả về |
| Nhìn thấy app | Có: cửa sổ MagicPic trên màn hình, xem agent sửa trực tiếp (chế độ bridge) | Không: máy cloud không có màn hình, chỉ có ảnh chụp |
| Token của model | Chỉ cho việc chỉnh ảnh | Cộng thêm phần agent điều khiển việc build và chuyển file |
| Mỗi lần chỉnh | Mở Claude/Codex là dùng được ngay | Chờ dựng lại môi trường |
| Hợp với | Chỉnh ảnh, xuất file, làm nhiều lần | Thêm tính năng, sửa lỗi, chạy test, mở pull request |

Ở cả hai cách, phần tốn token chính là các ảnh xem trước mà agent nhìn: với Claude, một ảnh
1024 px tốn cỡ một nghìn token (ước tính theo cách Anthropic tính ảnh, chưa đo trên MagicPic).
Muốn tiết kiệm, hãy bảo agent xem ảnh nhỏ hơn (`max_side` 768) và chỉ xem lại khi cần.

**Còn một cách nhẹ hơn nữa là tải file cài sẵn, nhưng hiện chưa có.** Quy trình đóng gói thừa
hưởng từ dự án gốc chưa được chạy cho MagicPic (xem [`docs/releasing.md`](../releasing.md)).
Làm bản phát hành đầu tiên là một quyết định riêng: nó công khai file cài lên GitHub.

## 2. Cách nhanh nhất: nhờ agent trên máy bạn tự cài

Mở **Claude Code** (hoặc **Codex CLI**) trên máy của bạn, ở một thư mục trống, rồi dán nguyên
đoạn dưới đây. Agent sẽ hỏi bạn trước những bước cần quyền quản trị.

```text
Cài MagicPic từ mã nguồn trên máy này và nối nó với chính bạn qua MCP. Làm từng bước, kiểm tra
kết quả từng bước, và hỏi tôi trước khi cài phần mềm hệ thống hoặc chạy gì với quyền quản trị.

1. Kiểm tra git, Rust (cargo/rustc 1.95 trở lên). Trên Windows còn cần Visual Studio Build Tools
   với workload "Desktop development with C++". Thiếu gì thì đề xuất lệnh cài (Windows:
   winget install Git.Git, Rustlang.Rustup, Microsoft.VisualStudio.2022.BuildTools) và chờ tôi đồng ý.
2. git clone https://github.com/vansyson1308/vansysoneditphoto magicpic
3. Trong thư mục magicpic: cargo install --path apps/magicpic-cli --locked
   (10–40 phút tùy máy; nếu Windows báo "An Application Control policy has blocked this file",
   dừng lại và đọc mục Smart App Control trong docs/development.md).
4. Tạo thư mục ảnh riêng: Windows C:\Users\<tên>\Pictures\MagicPic, macOS/Linux ~/Pictures/MagicPic.
   Chỉ cấp quyền cho đúng thư mục này, không cấp cả ổ đĩa hay thư mục home.
5. Đăng ký MCP server tên "magicpic" chạy: <đường dẫn magicpic-cli> mcp
   --automation-read-root <thư mục ảnh> --automation-write-root <thư mục ảnh>
   Claude Code: claude mcp add --transport stdio --scope user magicpic -- <lệnh và tham số trên>
   Codex: codex mcp add magicpic -- <lệnh và tham số trên>, rồi trong ~/.codex/config.toml
   đặt tool_timeout_sec = 300 cho [mcp_servers.magicpic].
6. Kiểm tra: liệt kê công cụ của server magicpic, rồi gọi files_list. Báo tôi đường dẫn file
   chạy, thư mục ảnh và lệnh đã dùng để đăng ký.
Hướng dẫn chi tiết: docs/vi/cai-dat-va-ket-noi.md và docs/vi/art-of-seeing-va-mcp.md trong repo.
```

Sau khi agent báo xong, **khởi động lại** Claude Code hoặc Codex để nó nạp server mới.
Với **Claude Desktop** (ứng dụng chat), bạn tự thêm cấu hình ở mục 3.

## 3. Tự cài bằng tay (Windows)

1. **Cài công cụ**, mỗi lệnh chạy một lần trong PowerShell:

   ```powershell
   winget install --id Git.Git
   ```

   ```powershell
   winget install --id Rustlang.Rustup
   ```

   ```powershell
   winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
   ```

   Mở một cửa sổ PowerShell **mới**, kiểm tra `cargo --version` (cần 1.95 trở lên).
2. **Tải mã nguồn và build CLI** (10–40 phút tùy máy):

   ```powershell
   git clone https://github.com/vansyson1308/vansysoneditphoto magicpic
   ```

   ```powershell
   cd magicpic; cargo install --path apps/magicpic-cli --locked
   ```

   File chạy nằm ở `%USERPROFILE%\.cargo\bin\magicpic-cli.exe`. Nếu Windows 11 chặn với lỗi
   *An Application Control policy has blocked this file*, đó là Smart App Control: xem
   [`docs/development.md`](../development.md#windows-source-builds).
3. **Tạo thư mục ảnh**, ví dụ `C:\Users\<bạn>\Pictures\MagicPic`. Agent chỉ đọc và ghi trong
   thư mục này, kể cả thư mục con nó tự tạo khi lưu.
4. **Đăng ký với client.** Cấu hình cho Claude Code, Claude Desktop và Codex (kèm chế độ bridge
   để agent sửa ảnh đang mở trong app) nằm ở
   [`art-of-seeing-va-mcp.md` mục 4](art-of-seeing-va-mcp.md#4-cài-đặt-trên-windows).
5. **App desktop** (chỉ cần nếu muốn xem cửa sổ MagicPic hoặc dùng bridge):

   ```powershell
   cargo build --release -p magicpic
   ```

   App nằm ở `target\release\magicpic.exe`.

macOS và Linux: cài Rust bằng [rustup](https://rustup.rs); Linux cần thêm các gói liệt kê ở
[`docs/development.md`](../development.md#prerequisites). Các lệnh còn lại như trên, đường dẫn
theo kiểu `~/…`.

## 4. Kiểm tra đã nối được chưa

Chép một ảnh vào thư mục ảnh, rồi nói với Claude hoặc Codex:

> Liệt kê ảnh trong thư mục MagicPic, mở ảnh mới nhất và cho tôi xem nó.

Agent phải gọi `files_list`, `doc_open` và `render_preview`, rồi cho bạn thấy ảnh. Nếu nó nói
không có công cụ MagicPic: khởi động lại client, rồi kiểm tra `/mcp` (Claude Code) hoặc
`codex mcp list`.

## 5. Cập nhật lên bản mới

Trong thư mục `magicpic`, chạy hai lệnh sau rồi khởi động lại Claude/Codex:

```powershell
git pull
```

```powershell
cargo install --path apps/magicpic-cli --locked --force
```

Build lần sau nhanh hơn lần đầu nhiều nếu không xóa thư mục `target`.

## 6. Đã kiểm chứng tới đâu

- **ĐÃ CHẠY** (Linux, máy cloud): đúng lệnh `cargo install` ở trên (10,5 phút), rồi qua MCP:
  `files_list`, `doc_open`, xem trước và xuất vào một thư mục con chưa có (`ket-qua/`); trước đó
  là một phiên Claude Code thật qua server `magicpic`. Trên Windows, CI của repo build và chạy toàn bộ test.
- **CHƯA KIỂM CHỨNG:** đoạn lời nhắn ở mục 2 chưa được chạy trên một máy Windows thật, và các
  lệnh `winget` chưa được thử; nếu một bước báo lỗi, đoạn lời nhắn đã dặn agent dừng lại và báo
  bạn. Phiên Codex và Claude Desktop thật cũng chưa chạy.
