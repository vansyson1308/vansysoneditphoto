# Cài MagicPic và nối với Claude hoặc Codex

Trang này trả lời hai câu hỏi: nên **cài MagicPic về máy** hay **dùng phiên Claude Code trên
cloud**, và cài thế nào. Cách dùng hằng ngày, lời nhắn mẫu và giới hạn đã biết nằm ở
[`art-of-seeing-va-mcp.md`](art-of-seeing-va-mcp.md).

## 1. Nên chọn cách nào

**Để chỉnh ảnh: cài về máy.** Phiên trên cloud (như phiên đã phát triển MagicPic) hợp để
**sửa mã nguồn của MagicPic**, không hợp để chỉnh ảnh hằng ngày.

| | Cài về máy, nối Claude/Codex trên máy | Phiên Claude Code trên cloud |
|---|---|---|
| Chuẩn bị | Build một lần: trên máy Linux 4 nhân đo được 10 phút cho CLI và thêm 15 phút cho app (3,4 GB thư mục build); máy yếu hơn hoặc Windows có thể lâu hơn | Mỗi phiên mới là một máy mới: tải mã nguồn và build lại (đo được khoảng 6 phút trên máy 4 nhân, chưa kể cài thư viện) |
| Ảnh của bạn | Nằm yên trên máy bạn | Phải tải lên máy cloud, rồi tải kết quả về |
| Nhìn thấy app | Có: cửa sổ MagicPic trên màn hình, xem agent sửa trực tiếp (chế độ bridge) | Không: máy cloud không có màn hình, chỉ có ảnh chụp |
| Token của model | Chỉ cho việc chỉnh ảnh | Cộng thêm phần agent điều khiển việc build và chuyển file |
| Mỗi lần chỉnh | Mở Claude/Codex là dùng được ngay | Chờ dựng lại môi trường |
| Hợp với | Chỉnh ảnh, xuất file, làm nhiều lần | Thêm tính năng, sửa lỗi, chạy test, mở pull request |

Ở cả hai cách, phần tốn token chính là các ảnh xem trước mà agent nhìn: với Claude, một ảnh
1024 px tốn cỡ một nghìn token (ước tính theo cách Anthropic tính ảnh, chưa đo trên MagicPic).
Muốn tiết kiệm, hãy bảo agent xem ảnh nhỏ hơn (`max_side` 768) và chỉ xem lại khi cần.

**Cách nhanh nhất là tải bản dựng sẵn**: không cần Rust, không build, chỉ một lệnh (mục 3).
Quy trình `agent-binaries` đã sẵn sàng: mỗi khi người duy trì gắn thẻ phiên bản (`v…`) và công
bố bản phát hành, GitHub đính kèm file nén cho Windows x64, macOS (Apple Silicon và Intel),
Linux x64, file `SHA256SUMS` và hai trình cài đặt. **Hiện chưa có bản nào được công bố**: cho
tới lúc đó, lệnh cài báo lỗi 404 và agent tự chuyển sang build từ mã nguồn
([`docs/agent-setup-source.md`](../agent-setup-source.md)). Công bố bản đầu tiên là quyết định
của người duy trì: nó công khai file cài lên GitHub. File chưa được ký số, nên Windows hoặc macOS
có thể hỏi lại (xem [`docs/agent-troubleshooting.md`](../agent-troubleshooting.md#unsigned-binaries)).

## 2. Prompt chuẩn: dán vào agent trên máy mới

Trên một máy mới, mở **Claude Code** hoặc **Codex** ở một thư mục bất kỳ rồi dán nguyên đoạn
dưới đây. Agent tự cài công cụ (hỏi bạn trước), tải mã nguồn, build, mở app và nối hai server
MCP vào chính nó. Dán lại đúng đoạn này về sau là **cập nhật** lên bản mới.

```text
Cài và mở MagicPic trên máy này, rồi nối nó với chính bạn qua MCP để tôi chỉnh ảnh bằng cách
nói chuyện với bạn.

Mã nguồn: https://github.com/vansyson1308/vansysoneditphoto
Làm đúng theo hướng dẫn dành cho agent:
https://raw.githubusercontent.com/vansyson1308/vansysoneditphoto/main/docs/agent-setup.md
Nếu không đọc được trang đó, hãy chạy `git clone --depth 1` với mã nguồn ở trên rồi đọc
docs/agent-setup.md trong bản clone.

Hỏi tôi trước khi cài phần mềm hệ thống hoặc dùng quyền quản trị. Chỉ cấp quyền cho một thư mục
ảnh. Không in token ra màn hình. Nói với tôi bằng tiếng Việt. Cuối cùng báo lại những gì đã làm
và câu tôi cần nói sau khi khởi động lại bạn.
```

Các bước chi tiết nằm trong [`docs/agent-setup.md`](../agent-setup.md), một trang tiếng Anh
viết cho agent. Prompt ngắn vì mọi chi tiết nằm trong repo: khi hướng dẫn được sửa, prompt vẫn
giữ nguyên.

Agent xong việc thì:

1. **Khởi động lại** Claude Code hoặc Codex để nó nạp hai server: `magicpic` (chỉnh file trong
   thư mục ảnh, không cần mở app) và `magicpic-app` (điều khiển cửa sổ MagicPic đang mở, bạn
   xem trực tiếp).
2. Chép ảnh vào thư mục ảnh (mặc định `Pictures\MagicPic` trong thư mục người dùng), rồi nói
   chẳng hạn: *"Mở ảnh mới nhất trong thư mục MagicPic trong app và cho tôi xem."*
3. Lần sau muốn mở app: chạy launcher mà agent đã tạo (`.magicpic\start-magicpic` trong thư mục
   người dùng), hoặc nhờ agent mở giúp.

Codex có thể xin phép truy cập mạng để tải file: hãy đồng ý. Với **Claude Desktop** (ứng dụng
chat), agent chạy `mcp register --client claude-desktop --restart` rồi nhờ bạn **thoát hẳn**
Claude Desktop (biểu tượng ở khay hệ thống › Quit): MagicPic ghi cấu hình rồi tự mở lại Claude
sau vài giây. Claude Desktop ghi đè cấu hình khi đang chạy, nên không sửa file lúc nó đang mở.

## 3. Tự cài bằng tay

**Từ bản dựng sẵn (khi đã có bản phát hành), bốn lệnh.** Windows, trong PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.ps1 | iex"
```

macOS và Linux:

```sh
curl -fsSL https://github.com/vansyson1308/vansysoneditphoto/releases/latest/download/install.sh | sh
```

Lệnh in ra một đoạn JSON có đường dẫn `cli` (ví dụ `C:\Users\<bạn>\.magicpic\bin\magicpic-cli.exe`).
Rồi chạy, với đường dẫn đó:

```powershell
magicpic-cli launch
```

```powershell
magicpic-cli mcp register --client claude-code
```

(`--client codex`, hoặc `--client claude-desktop --restart`.)

```powershell
magicpic-cli doctor
```

`doctor` phải báo `"ok": true`; nếu không, mục `problems` ghi lỗi và lệnh sửa.

**Từ mã nguồn (Windows):**

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
2. **Tải mã nguồn và build cả hai chương trình một lần** (15–40 phút tùy máy):

   ```powershell
   git clone --depth 1 https://github.com/vansyson1308/vansysoneditphoto magicpic
   ```

   ```powershell
   cd magicpic; cargo build --release --locked -p magicpic -p magicpic-cli
   ```

   Nếu Windows 11 chặn với lỗi *An Application Control policy has blocked this file*, đó là
   Smart App Control: xem [`docs/development.md`](../development.md#windows-source-builds).
3. **Chép hai file** `target\release\magicpic.exe` và `target\release\magicpic-cli.exe` vào
   `%USERPROFILE%\.magicpic\bin\`. Sau đó có thể chạy `cargo clean` để lấy lại dung lượng.
4. **Chạy `magicpic-cli setup`** (tạo thư mục ảnh `Pictures\MagicPic`, launcher và cấu hình),
   rồi `launch`, `mcp register` và `doctor` như phần bản dựng sẵn ở trên.

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

Bản dựng sẵn: chạy lại lệnh cài ở mục 3. Nó thay hai chương trình trong `.magicpic\bin` và
giữ nguyên thư mục ảnh, launcher và đăng ký; thoát MagicPic trước (file đang chạy không thay
được). Hoặc dán lại prompt ở mục 2.

Từ mã nguồn: trong thư mục `magicpic`, `git pull`, build lại như mục 3, chép lại hai file vào
`.magicpic\bin`, rồi khởi động lại Claude/Codex. Build lần sau nhanh hơn nhiều nếu chưa xóa
thư mục `target`.

## 6. Đã kiểm chứng tới đâu

- **ĐÃ CHẠY trên một máy Linux sạch ở cloud (2026-11-12):** làm đúng từng bước của
  [`docs/agent-setup.md`](../agent-setup.md): clone, build CLI (10 phút) và app (15 phút), tạo
  thư mục ảnh và launcher. App mở lần đầu thì tự tạo token (quyền `600`, không in ra log), và
  `claude mcp list` báo cả `magicpic` lẫn `magicpic-app` là *Connected*. Sau đó một phiên Claude
  Code thật, chỉ được dùng hai server này, nhận câu *"Mở ảnh mới nhất trong thư mục MagicPic
  trong app và cho tôi xem"*: nó tìm ảnh, mở trong cửa sổ app và mô tả đúng bức ảnh.
- **Chưa chạy:**
  - Các bước trên do tôi tự làm theo hướng dẫn, chưa có agent nào nhận nguyên prompt trên một
    máy mới: môi trường thử không cho chạy một agent con có quyền chạy lệnh tùy ý.
  - Windows (`winget`, launcher PowerShell, `Test-NetConnection`) và macOS chưa thử. Trên
    Windows, CI của repo build và chạy toàn bộ test.
  - Phiên Codex và Claude Desktop thật chưa chạy.

  Nếu một bước báo lỗi, hướng dẫn dặn agent dừng lại và báo bạn.
- **Cách cài mới (2026-11-12):** `install.sh`, `magicpic-cli setup`, `mcp register` (Claude Code,
  Codex, Claude Desktop qua file chỉ định) và `doctor` đã chạy trên Linux, cả với chương trình
  giả lẫn CLI thật; CI chạy `install.ps1` trong Windows PowerShell 5.1 và cài thử trên Windows,
  macOS, Linux từ chính các file nén vừa build. Phần `--restart` của Claude Desktop (chờ thoát,
  mở lại qua `shell:AppsFolder`) chưa chạy trên một máy Windows thật.
