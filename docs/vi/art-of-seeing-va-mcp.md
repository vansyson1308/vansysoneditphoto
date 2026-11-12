# MagicPic: "nghệ thuật thấy" và chỉnh ảnh bằng Claude / Codex qua MCP

Tài liệu này dành cho người dùng không chuyên lập trình. Phần kỹ thuật (tiếng Anh) nằm ở
[`docs/seeing.md`](../seeing.md) và [`docs/mcp.md`](../mcp.md).

Mỗi nhận định dưới đây ghi rõ mức kiểm chứng:

- **ĐÃ CHẠY**: đã chạy thật và xem kết quả.
- **ĐÃ TEST**: có kiểm thử tự động trong repo chạy đạt.
- **THEO TÀI LIỆU**: lấy từ tài liệu chính thức của hãng, chưa tự chạy trên máy đó.
- **CHƯA KIỂM CHỨNG**: chưa có điều kiện chạy; có ghi bước cần làm.

## 1. Bạn làm được gì

1. Nối Claude hoặc Codex với MagicPic, mở ảnh, nói điều bạn muốn bằng tiếng Việt.
2. Agent **nhìn** ảnh (kể cả từng vùng, và qua năm "chế độ quan sát"), chỉnh bằng chính engine
   của MagicPic, xem lại, sửa tiếp theo lời bạn.
3. Thử nhiều cách thể hiện trên **bản sao**; ảnh gốc không bị đụng tới. Mỗi bản sao kèm một
   **recipe** (công thức) để làm lại y hệt.
4. Lưu project chỉnh tiếp được (`.pcraft`, giữ layer/mask) và xuất ảnh phẳng (`.jpg`, `.png`).
   Hai việc này tách bạch; xuất ảnh không làm mất layer trong project.

## 2. Từ PDF đến tính năng (bảng truy vết)

Nguồn: bản PDF 15 trang "Photography and the Art of Seeing" (Freeman Patterson, Đặng Quang
Hiếu dịch). Đây là **trích đoạn** (các phần "Những trở ngại trong sự thấy", "Học cách quan
sát", "Suy nghĩ phi chính thống", "Sự tình cờ", "Những thứ quen thuộc", "Khả năng linh hoạt",
"Tập trung trong thư giãn"), không phải toàn bộ sách. PDF không nằm trong repo và không có
đoạn nào được chép lại; số trang là trang của file PDF.

Ba cột đầu phân biệt rõ: **PDF nói gì** (diễn đạt lại ngắn), **tôi hiểu thế nào** (diễn
giải), **tính năng** (do tôi suy ra, không phải ý của tác giả).

| Ý trong PDF (trang) | Diễn giải | Vấn đề của người dùng | Tính năng và hành vi | Lệnh / API | Kiểm thử | Giới hạn |
|---|---|---|---|---|---|---|
| Gắn nhãn làm ta thôi không thấy; nhìn cây dương xỉ thì hãy thấy hình tam giác, sợi lá, các sắc xanh (tr. 3) | Tách một thuộc tính thị giác ra khỏi "tên" của vật | Nhìn ảnh chỉ thấy "cô gái", "ngọn núi", không thấy cấu trúc | Năm chế độ quan sát: chỉ sắc độ, khối màu, mảng hình, đường nét, màu không sắc độ | `seeing.view`, MCP `render_preview {view}` | Unit test từng view; test "xem không đổi tài liệu" ở 8/16/32-bit | Tính trên bản thu nhỏ (≤ 2048 px), là công cụ quan sát chứ không phải phép đo màu |
| Thả lỏng mắt cho vật mờ đi, để ý màu của khối hình đã mờ (tr. 14) | Làm mờ có chủ đích để thấy khối màu | Chi tiết che mất bố cục màu | View `masses` (mờ 1–5 % cạnh dài, chỉnh bằng `strength`) | như trên | Test: sọc 1 px biến mất, hai nửa sáng/tối vẫn tách; màu không lem từ vùng trong suốt | Không phải mô phỏng ống kính |
| Khoảng không giữa những chiếc lá tạo nên hình gì? (tr. 14) | Thấy "hình" và "nền" như hai mảng | Khó đánh giá khoảng trống | View `shapes`: 2–8 mảng sắc độ phẳng theo chính dải sáng tối của ảnh | như trên | Test: đúng số mảng; ảnh phẳng vẫn phẳng | Ngưỡng theo phân vị 2–98 % của ảnh, không so sánh được giữa hai ảnh |
| Nhìn đường nét mà không để ý chi tiết bề mặt; hình dung đường ven, bỏ qua hoa văn và độ sâu (tr. 14) | Tách đường viền khỏi texture | Texture làm rối đường dẫn mắt | View `lines`: nét đậm trên nền trắng, bỏ nhiễu texture | như trên | Test: cạnh lớn có nét, hạt nhiễu ±2 % không có nét, ảnh phẳng là giấy trắng | Chỉ là biên theo sắc độ; không "hiểu" vật thể |
| Các sắc màu khác nhau; màu sắc và tâm trạng (tr. 3, 14, 15) | (Suy ra) xem quan hệ màu khi bỏ sáng tối | Sáng tối lấn át cảm nhận màu | View `color` (mọi điểm cùng độ sáng) và `luminance` (chỉ sắc độ) | như trên | Test: hai xám khác sắc độ thành một; ô đỏ vẫn đỏ | `luminance`/`color` là suy ra của tôi, PDF không nêu trực tiếp |
| Tung vòng dây, nghiên cứu mảnh đất trong vòng; chụp ≥ 20 tấm trong một vòng tròn nhỏ (tr. 9) | Để ngẫu nhiên chọn chỗ nhìn, thay cho thói quen | Luôn crop theo một kiểu | Khung ngẫu nhiên **có seed**: cùng seed ra cùng khung, để tìm lại được | `seeing.frames`, `render_preview {region}` | Test: lặp lại theo seed, luôn nằm trong ảnh, đúng tỉ lệ | Chỉ chọn vùng trong ảnh đã có; không thay được việc đổi chỗ đứng |
| Lập danh sách quy tắc rồi phá: giữ chắc máy, chủ thể nét, đúng sáng; thừa/thiếu sáng 1–3 EV; rung/xoay máy; chụp mờ (tr. 7–8) | Thử nghiệm ngược quy tắc để có điểm khởi đầu mới | Sợ "làm hỏng" ảnh | Thử nghiệm phá quy tắc trên bản sao: ±1–3 EV (adjustment layer), rung dọc/lia ngang/xoay/zoom và mất nét (smart filter) | `seeing.experiments`, `seeing.variant` | Test: mọi kiểu chạy ở 8/16/32-bit, nguồn không đổi, vẫn chỉnh lại được | Là mô phỏng trên ảnh đã chụp, không tái tạo chuyển động thật của máy |
| Chụp nhiều, đừng vội xóa; sự tình cờ gợi ý ý tưởng mới (tr. 8–9) | Giữ kết quả, làm lại được | Thử xong không nhớ đã làm gì | Mỗi biến thể lưu **recipe** (seed, khung, các bước) trong chính file; chạy lại recipe ra đúng điểm ảnh cũ | `seeing.recipe`, `seeing.variant {recipe}` | Test: replay bằng nhau tuyệt đối; còn nguyên sau lưu và mở lại `.pcraft` | Recipe gắn với ảnh nguồn; đổi ảnh nguồn thì kết quả khác |
| Xem kết quả: thả lỏng cảm xúc trước ("tôi thích nó!"), rồi mới hỏi vì sao; bạn thích nhất phần nào, vì sao (tr. 8, 15) | Ghi phản ứng của chính người xem, máy không chấm điểm | Chọn xong quên lý do | Ghi phản ứng và lý do **do bạn nói** vào biến thể | `seeing.note` | Test: lưu thành ghi chú, Undo gỡ được | Không có "điểm đẹp", không tự chọn hộ |
| "Chính thống / phi chính thống": ảnh trống một nửa thì lùi ra xa hơn; ảnh lộn xộn thì thể hiện texture (tr. 11–12) | Đừng mặc định "lỗi" phải sửa | Công cụ tự động hay "sửa" mất ý đồ | Nguyên tắc thiết kế: không auto-fix, không coi mờ/tối/khoảng trống là lỗi; prompt mẫu gợi ý agent đưa cả cách đọc ngược lại | (nguyên tắc, mục 7) | Xem lại bằng mắt | Phụ thuộc cách agent diễn giải lời bạn |
| Máy ảnh ghi tất cả, con người trừu tượng hóa (tr. 4) | Cần công cụ giúp lược bớt | — | Chính là các view ở trên | — | — | — |
| Tập trung vào một chi tiết rồi lại nhìn tổng thể (tr. 14) | Chuyển giữa tổng thể và chi tiết | Zoom mất bối cảnh | Xem một vùng ở đúng kích thước thật (`region`), cạnh ảnh tổng thể | `render_preview {region}` | Test: vùng không bị phóng to, sai vùng thì báo lỗi | Trong app dùng zoom/screen mode sẵn có; chưa làm chế độ "ít phân tâm" riêng |
| Thư giãn, đổi vị trí chụp, đổi hướng ánh sáng, chụp theo nhóm (tr. 5–6, 9–10, 13–14) | Việc của người chụp ngoài đời | — | **Không làm**: phần mềm không tái tạo được góc nhìn hay ánh sáng không có trong ảnh | — | — | Nêu rõ để không hứa quá |

**Tư duy ngược** (đi từ trải nghiệm về kỹ thuật): "xem ba cách nhìn rồi chọn" cần (1) nhìn
được mà không sửa, (2) bản sao rẻ, (3) làm lại được, (4) hỏng giữa chừng không để lại rác.
Bốn điều đó thành: view chỉ-đọc, `seeing.variant` trên bản sao, recipe có seed, và giao dịch
"được ăn cả, ngã về không".

**Tư duy phản biện** (các phản ví dụ đã cân nhắc):

- *View có thể bị xuất nhầm thành ảnh?* Không: view chỉ trả về hình để nhìn; muốn giữ phải
  chủ động mở thành "study" là một tài liệu riêng, tên có chữ `study`.
- *Biến thể ngẫu nhiên có thể chỉ là preset rẻ tiền gắn tên sách?* Vì vậy không có preset
  "phong cách": mỗi thử nghiệm phá đúng một quy tắc nêu trong PDF, tham số do seed chọn trong
  khoảng hẹp, và agent hoặc bạn tự viết recipe khác được.
- *Một giao diện đơn giản hơn đã đủ?* Với người dùng app: có, menu **View › Seeing** chỉ sáu
  mục. Phần còn lại dành cho agent.
- *Mờ/tối làm ảnh xấu đi?* Có thể. Vì thế chỉ làm trên bản sao và không bao giờ tự áp vào
  bản chính.

## 3. Ma trận client

| Client | Kết nối | Đưa ảnh vào | Agent thấy preview? | Nhận file | Trạng thái |
|---|---|---|---|---|---|
| **Claude Code** (CLI; cũng là tab Code của Claude Desktop) | stdio cục bộ, `claude mcp add` hoặc `.mcp.json` | Lưu ảnh vào thư mục được cấp quyền (agent tìm bằng `files_list`), hoặc đưa đường dẫn | Có: ảnh trả về là image content | File nằm trong thư mục ghi; reply có `absolutePath` | **ĐÃ CHẠY** trên Linux (v2.1.322), cả headless và bridge; trên Windows: THEO TÀI LIỆU |
| **Claude Desktop** (chat) | stdio cục bộ, file `claude_desktop_config.json` (Settings › Developer › Edit Config) | Như trên. Ảnh **dán vào khung chat không được chuyển cho MCP server** (không có tài liệu nào nói là có) | THEO TÀI LIỆU: hỗ trợ kết quả dạng ảnh; giới hạn kết quả ~150.000 ký tự được ghi cho connector, nên dùng `format: "jpeg"` và `max_side` nhỏ | Như trên | CHƯA KIỂM CHỨNG trên máy thật |
| **Codex CLI / IDE / app ChatGPT desktop** | stdio cục bộ, `~/.codex/config.toml` hoặc `codex mcp add` | Như trên; `codex -i ảnh.jpg` chỉ cho model xem, nên vẫn cần ghi đường dẫn trong lời nhắn | THEO MÃ NGUỒN Codex: image content được chuyển cho model; không có tài liệu chính thức | Như trên | Cấu hình: ĐÃ CHẠY (`codex mcp add`, v0.167.2). Phiên thật: **CHƯA KIỂM CHỨNG** (cần bạn đăng nhập Codex) |
| **claude.ai web, ChatGPT web, Codex cloud** | Không nối được server cục bộ (cần HTTPS công khai hoặc tunnel của hãng) | — | — | — | Ngoài phạm vi; không mở cổng máy bạn ra Internet |

Hai chế độ của MagicPic:

- **Headless** (`magicpic-cli mcp --automation-read-root … --automation-write-root …`):
  không cần mở app. Phù hợp "đưa ảnh, nhận ảnh".
- **Bridge** (`magicpic --control …` rồi `magicpic-cli mcp --bridge …`): agent sửa đúng
  tài liệu **đang mở trong app** của bạn; bạn thấy thay đổi ngay và có thể sửa tay xen kẽ.
  Nếu app chưa chạy, mọi lệnh báo lỗi "cannot connect … not sent"; server **không** tự chuyển
  sang headless để rồi sửa nhầm một phiên khác (ĐÃ TEST).

## 4. Cài đặt trên Windows

Chưa có bản cài sẵn cho nhánh này, nên cần build từ mã nguồn một lần (khoảng 20–40 phút).

1. Cài **Rust** theo [hướng dẫn của Microsoft](https://learn.microsoft.com/en-us/windows/dev-environment/rust/setup):
   Visual Studio Build Tools với workload *Desktop development with C++*, rồi `rustup`.
2. Mở PowerShell tại thư mục mã nguồn và cài CLI (và app nếu dùng bridge):

   ```powershell
   cargo install --path apps/magicpic-cli --locked
   ```

   ```powershell
   cargo build --release -p magicpic
   ```

   CLI nằm ở `%USERPROFILE%\.cargo\bin\magicpic-cli.exe`; app ở `target\release\magicpic.exe`.
3. Tạo một thư mục riêng cho ảnh, ví dụ `C:\Users\<bạn>\Pictures\MagicPic`. Agent chỉ đọc
   và ghi được **trong thư mục này**. Đừng cấp cả ổ đĩa.

### Claude Code

```powershell
claude mcp add --transport stdio --scope user magicpic -- "%USERPROFILE%\.cargo\bin\magicpic-cli.exe" mcp --automation-read-root "C:\Users\<bạn>\Pictures\MagicPic" --automation-write-root "C:\Users\<bạn>\Pictures\MagicPic"
```

Trong PowerShell hãy thay `%USERPROFILE%` bằng đường dẫn đầy đủ (hoặc `$env:USERPROFILE`).
Kiểm tra bằng `/mcp` trong Claude Code.

### Claude Desktop

Mở **Settings › Developer › Edit Config** (đừng tự đoán vị trí file; bản cài MSIX đặt ở chỗ
khác) và thêm:

```json
{
  "mcpServers": {
    "magicpic": {
      "command": "C:\\Users\\<bạn>\\.cargo\\bin\\magicpic-cli.exe",
      "args": ["mcp",
               "--automation-read-root", "C:\\Users\\<bạn>\\Pictures\\MagicPic",
               "--automation-write-root", "C:\\Users\\<bạn>\\Pictures\\MagicPic"]
    }
  }
}
```

Thoát hẳn Claude Desktop rồi mở lại. Đường dẫn phải là đường dẫn tuyệt đối, dấu `\` viết đôi.

### Codex (CLI, IDE, app desktop dùng chung một file)

File `%USERPROFILE%\.codex\config.toml`:

```toml
[mcp_servers.magicpic]
command = 'C:\Users\<bạn>\.cargo\bin\magicpic-cli.exe'
args = ["mcp",
        "--automation-read-root", 'C:\Users\<bạn>\Pictures\MagicPic',
        "--automation-write-root", 'C:\Users\<bạn>\Pictures\MagicPic']
startup_timeout_sec = 20
tool_timeout_sec = 300
```

`tool_timeout_sec` mặc định của Codex là 60 giây; nâng lên vì một số bộ lọc trên ảnh lớn chạy
lâu hơn. Schema này khác của Claude (TOML, khóa `mcp_servers`), đừng chép JSON của Claude sang.
Nếu chạy Codex trong WSL thì nó dùng file cấu hình và đường dẫn của Linux, không dùng `.exe`.

### Bridge: agent sửa ảnh đang mở trong app

1. Mở app kèm kênh điều khiển (file token tự được tạo, chỉ nghe trên máy bạn):

   ```powershell
   target\release\magicpic.exe --control 7878 --control-token-file "C:\Users\<bạn>\.magicpic-control.token" --automation-read-root "C:\Users\<bạn>\Pictures\MagicPic" --automation-write-root "C:\Users\<bạn>\Pictures\MagicPic"
   ```

2. Trong cấu hình client, thay `args` bằng:
   `["mcp", "--bridge", "127.0.0.1:7878", "--control-token-file", "C:\\Users\\<bạn>\\.magicpic-control.token"]`.
   Quyền thư mục khi đó do **app** giữ; không truyền `--automation-*-root` cho CLI.

Token là bí mật của máy bạn: đừng dán vào chat, đừng đưa lên repo.

## 5. "Dán ảnh vào chat là chỉnh được": sự thật

Ảnh bạn dán vào khung chat, ảnh trong thư mục máy bạn và ảnh mà MCP server đọc được là **ba
thứ khác nhau**. Không client nào trong mục 3 được tài liệu xác nhận là chuyển ảnh dán trong
chat cho MCP server cục bộ. Vì vậy:

- **Ít thao tác nhất, dùng được ở mọi client:** lưu ảnh vào thư mục đã cấp quyền (kéo-thả
  trong Explorer), rồi nói *"mở ảnh mới nhất"*. Agent gọi `files_list` (xếp mới nhất trước)
  và `doc_open`.
- **Gần với "dán là chỉnh" nhất:** dùng bridge. Copy ảnh, vào MagicPic chọn
  **File › New from Clipboard** (hoặc kéo ảnh vào cửa sổ), rồi nói với agent *"chỉnh ảnh đang
  mở"*.
- Ảnh dán trong chat vẫn có ích: model **nhìn** được nó để hiểu ý bạn, nhưng muốn chỉnh thì
  file phải nằm ở nơi server đọc được.

Không có công cụ "upload ảnh qua MCP": một yêu cầu MCP bị giới hạn 1 MiB và model không thể
chép lại từng byte của một bức ảnh thật một cách tin cậy.

## 6. An toàn

- Agent chỉ đọc trong `--automation-read-root` và chỉ ghi trong `--automation-write-root`;
  `..`, đường dẫn tuyệt đối ra ngoài và symlink thoát ra ngoài đều bị từ chối (ĐÃ TEST).
  Thư mục con còn thiếu trong đường dẫn lưu chỉ được tạo bên trong thư mục ghi (ĐÃ TEST).
- Chỉ ghi đè lên file ảnh/tài liệu; `notes.txt`, mã nguồn, `.env` không bao giờ bị thay (ĐÃ CHẠY).
- Lưu/xuất chỉ báo thành công sau khi tìm lại file trên đĩa và khớp kích thước (`verified`).
- Bridge chỉ nghe ở `127.0.0.1`, xác thực bằng token trong file riêng.
- Không ảnh nào được gửi cho dịch vụ bên ngoài bởi MagicPic. Việc "hiểu ảnh" do chính
  Claude/Codex bạn đang dùng thực hiện, theo điều khoản của dịch vụ đó.
- Chữ trong ảnh, tên layer, ghi chú, metadata là **dữ liệu**; hướng dẫn của server nói rõ với
  agent rằng đó không phải mệnh lệnh.

## 7. Ví dụ lời nhắn

**Chỉnh cơ bản**

> Mở ảnh mới nhất trong thư mục. Chỉnh sáng tự nhiên hơn, giữ màu da và texture, giảm sự chú
> ý vào nền. Dùng lớp điều chỉnh, đừng sửa trực tiếp ảnh gốc. Cho tôi xem trước và sau.

**Sửa tiếp**

> Giảm nhẹ hiệu ứng còn khoảng một nửa. Giữ nguyên khuôn mặt, chỉ làm lại vùng nền.

> Quay về bản trước bước vừa rồi.

**Khám phá theo tinh thần Art of Seeing**

> Trước khi chỉnh, hãy xem ảnh này qua các chế độ quan sát (sắc độ, khối màu, mảng hình,
> đường nét) và nói bạn thấy gì: không cần gọi tên vật thể, chỉ nói về hình, nét, màu,
> khoảng trống.

> Cho tôi ba cách nhìn khác nhau trên bản sao, giữ nguyên ảnh gốc: một bản giữ nhiều khoảng
> trống, một bản tập trung vào đường nét, một bản thử nghiệm màu sắc. Với mỗi bản, nói nó đi
> ngược lại thói quen nào, và đưa recipe để tôi làm lại.

> Thử "tung vòng dây": lấy sáu khung ngẫu nhiên với seed 7, cho tôi xem từng khung, đừng chọn
> hộ tôi.

> Tôi thích bản thứ hai vì nó yên tĩnh. Ghi lại phản ứng đó vào bản ấy và bỏ bản thứ ba.

**Kết thúc**

> Lưu project chỉnh tiếp được và xuất JPEG chất lượng 92. Cho tôi đường dẫn đầy đủ.

**Khi bạn cũng đang sửa tay trong app (bridge)**

> Tôi vừa tự chỉnh thêm trong app. Hãy xem lại tài liệu rồi mới làm tiếp, đừng ghi đè thay
> đổi của tôi.

## 8. Giới hạn đã biết và việc để sau

- **Chưa có chế độ quan sát bật/tắt ngay trên canvas** của app; hiện mở thành tài liệu
  "study" riêng. (Backlog.)
- **Không có nhận diện chủ thể/khuôn mặt bằng AI** trong các lệnh mới; "giữ nguyên khuôn mặt"
  dựa vào việc agent nhìn ảnh rồi tạo vùng chọn/mask bằng các lệnh sẵn có. Kết quả phụ thuộc
  model bạn dùng.
- **Giao dịch nguyên tử chạy liền một mạch**: không có thanh tiến độ, không hủy giữa chừng;
  bộ lọc rất nặng trên ảnh rất lớn có thể vượt 55 giây chờ của bridge (thao tác vẫn hoàn tất
  hoặc tự hoàn tác trong app; hãy xem lại tài liệu trước khi thử lại).
- **Kiểm tra "đúng tài liệu, đúng phiên bản"** (`expect_document` + `expect_revision`) là
  nguyên tử trong app, nhưng agent phải chủ động dùng; lời nhắn mẫu ở trên nhắc nó. Trong phiên
  thử với Claude Code (headless, một mình agent sửa) agent đã không dùng, và ở đó không cần.
- **Rollback không hoàn lại** clipboard, màu/cọ đang chọn của công cụ và file đã ghi ra đĩa.
- **Trong app, khi đang mở khung Free Transform hoặc Crop**, hãy xong hoặc hủy khung đó trước
  khi để agent chạy một giao dịch (các bước đi thẳng vào engine, không qua xử lý của khung).
- **Ảnh rộng gam màu** (Adobe RGB, ProPhoto) trong preview trông nhạt hơn một chút so với
  trong app vì hình preview không gắn profile; file xuất thì vẫn đúng profile.
- **Preview tối đa 2048 px**; ảnh nguồn trên 64 MP chỉ xem được theo vùng ở chế độ headless.
- **JPEG hỏng phần đuôi vẫn mở được phần đọc được**, kèm cảnh báo (hành vi có sẵn của
  MagicPic); PNG hỏng và file không phải ảnh thì bị từ chối.
- **Thư mục con chưa có sẽ được tạo** (đã sửa, 2026-11-12). Bảo agent xuất vào
  `ket-qua/anh.jpg` khi chưa có thư mục `ket-qua` thì `doc_save`/`doc_export` tự tạo
  `ket-qua` (và các thư mục cha còn thiếu) **bên trong** `--automation-write-root`, rồi ghi
  file như thường; ở headless, reply vẫn có `absolutePath`, `bytes`, `verified`. Bridge cũng
  tạo thư mục như vậy (app ghi qua cùng một hàm; chưa có test riêng cho bridge). Không bao giờ tạo thư mục ngoài thư mục ghi: `..`, đường dẫn tuyệt đối ra ngoài và
  symlink thoát ra ngoài vẫn bị từ chối; nếu một phần của đường dẫn là file có sẵn (ví dụ
  `notes.txt/anh.jpg`) thì báo lỗi (ĐÃ TEST). Một lần lưu thất bại sau khi đã tạo thư mục có
  thể để lại thư mục rỗng đó.
- Recipe lưu trong ghi chú của tài liệu: ai xóa ghi chú đó thì mất recipe.
- Không tuyên bố tương đương Photoshop; đây là một lát cắt nhỏ, dùng được, có kiểm chứng.

## 9. Kết quả kiểm chứng

Máy thử: Linux (Ubuntu 24.04) trong máy ảo 4 vCPU, 15 GB RAM, **không có GPU** (app chạy bằng
bộ dựng hình phần mềm llvmpipe), bản build `dev` (chưa tối ưu như bản `release`). **Chưa thử
trên Windows và macOS.**

| Kịch bản | Cách thử | Kết quả |
|---|---|---|
| A. Nhập → chỉnh tone → xem trước → xuất → mở lại file xuất | MCP thật qua stdio, ảnh chân dung 3840×4548 | ĐÃ CHẠY: đạt; file xuất mở lại đúng kích thước; vùng mặt của file xuất khớp bản xem trước |
| B. Chỉnh cục bộ bằng mask | Làm tối bầu trời qua mask mềm; đọc điểm ảnh ngoài mask trước và sau | ĐÃ CHẠY: đạt; điểm ngoài mask không đổi một bit |
| C. Ba biến thể, lưu recipe, tái tạo | `seeing.experiments` → `seeing.variant` → `seeing.recipe` → chạy lại | ĐÃ CHẠY + ĐÃ TEST: đạt; sai số định nghĩa là **0** (ảnh xem trước giống từng byte), còn nguyên sau lưu và mở lại `.pcraft` |
| D. Undo/redo, lưu và mở lại project | Lớp điều chỉnh + ghi chú, undo/redo, lưu `.pcraft`, mở lại | ĐÃ CHẠY: đạt; layer và ghi chú còn nguyên, ảnh dựng lại giống hệt |
| E. Bridge sửa đúng tài liệu trong app | App thật chạy dưới màn hình ảo; kiểm tra trạng thái và ảnh chụp cửa sổ | ĐÃ CHẠY: đạt; hai lớp của agent hiện trong bảng Layers; sửa tay xen kẽ làm lệnh cũ của agent bị từ chối; batch lỗi hoàn tác sạch |
| F. Phiên chỉnh ảnh thật trên client | **Claude Code 2.1.322** (Linux): headless 3 lượt (60 lệnh, 20 ảnh xem trước, 0 lỗi) và bridge 2 lượt (18 lệnh) | ĐÃ CHẠY với Claude Code; ở lần chạy lại, đo trên file xuất: vùng mặt sáng hơn 11–12,5 mức (thang 255), nền tối hơn khoảng 4 mức và độ rực màu của nền giảm từ 13,2 xuống 7,7 (xem ghi chú bên dưới về lần chạy đầu). **Codex CLI 0.167.2**: `codex mcp add` nhận cấu hình (ĐÃ CHẠY), nhưng chưa chạy được phiên thật vì chưa đăng nhập: **CHƯA KIỂM CHỨNG**. **Claude Desktop**: **CHƯA KIỂM CHỨNG** (cần Windows/macOS) |
| G. Lỗi giữa batch, mất phản hồi, kết nối lại, input sai, xuất không có quyền | Kịch bản MCP thật + test tự động (app giả lập ngắt kết nối giữa chừng) | ĐÃ CHẠY + ĐÃ TEST: đạt; không lần nào phá tài liệu, không ghi ra ngoài thư mục, không gửi lại lệnh sửa |
| H. Tính năng từ PDF gọi được qua MCP và dùng trong app | 5 view × 5 ảnh qua MCP; menu **View › Seeing** trong app thật | ĐÃ CHẠY: đạt; sáu mục menu có mặt và bấm được |

**Sau khi đổi tên thành MagicPic** (2026-11-12) các kịch bản được chạy lại với `magicpic-cli`
và app `magicpic`: A, B, C, D, G, H và phép đo 24 MP qua MCP thật, E với app thật dưới màn
hình ảo, và một lượt F với Claude Code 2.1.322 (12 lệnh, 4 ảnh xem trước, 0 lỗi; file JPEG và
`.pcraft` lưu ra mở lại đúng, hai lớp điều chỉnh còn nguyên): tất cả đạt. Một project `.pcraft`
do bản trước khi đổi tên lưu vẫn mở được.

Thời gian đo trên ảnh 24 MP (6000×4000), bản build `dev` nên bản `release` sẽ nhanh hơn; đây
là số đo, không phải cam kết: mở ảnh 0,2 s; mỗi chế độ quan sát ở 1024 px 0,1–0,3 s; xem một
vùng 1500×1500 ở kích thước thật 0,1 s; biến thể thừa/thiếu sáng dưới 0,05 s; biến thể mất
nét 3,1 s; biến thể rung máy dọc 15,6 s (bộ lọc Motion Blur sẵn có, khoảng cách lớn; 23 s khi máy đang bận việc khác); xuất
JPEG 2,3 s; lưu `.pcraft` 1,3 s; bộ nhớ đỉnh của server 1,8 GB khi mở cùng lúc 5 tài liệu 24 MP.

Ba điều học được từ các phiên Claude Code thật:

- **Lời agent báo cáo chưa phải bằng chứng.** Ở lần chạy đầu, agent báo "đã làm sáng khuôn
  mặt", nhưng người kiểm tra ảnh độc lập đo lại thì mặt **không sáng hơn**: lớp "làm tối nền"
  không có mask nên phủ lên cả ảnh, vì lớp điều chỉnh tạo trước đó đã dùng mất vùng chọn
  (MagicPic, cũng như Photoshop, biến vùng chọn thành mask của lớp mới rồi bỏ chọn). Đã sửa
  bằng cách đưa lời dặn này vào hướng dẫn mà server gửi cho agent; ở lần chạy lại agent chọn
  lại vùng trước khi thêm lớp và kết quả đo đúng yêu cầu. Bài học cho bạn: hãy nhìn ảnh, và
  khi nghi ngờ hãy bảo agent "đo lại vùng mặt trước và sau".
- Agent tự tìm ảnh mới nhất bằng `files_list`, tự dùng các chế độ quan sát để quyết định
  crop, và tự sửa vùng chọn chủ thể khi thấy nó sai. Chất lượng phần này phụ thuộc model.
- Ở lượt bridge, agent muốn "undo hai bước nhưng chỉ khi bạn chưa sửa gì"; bản đầu của tôi từ
  chối và agent phải xóa lớp thay thế. Đã sửa: undo/redo có kiểm tra giờ chạy được (đã thử
  lại trên app thật).

Mẹo khi chỉnh cục bộ: để làm mềm viền vùng chọn, hãy bảo agent dùng **Select › Modify ›
Feather** (`select.modify.feather`), cách này giữ nguyên độ phủ ở sát mép ảnh; tùy chọn
`feather` của công cụ chọn hình chữ nhật làm nhạt cả những cạnh trùng mép ảnh.

Ảnh trước/sau và ảnh chụp cửa sổ app được gửi kèm phiên làm việc và mô tả trong pull request
(theo quy định của repo, ảnh minh chứng không nằm trong repo).

## 10. Quay lui

Toàn bộ thay đổi nằm trong một pull request. Nếu muốn bỏ: không merge, hoặc revert commit đó.
Các lệnh mới không đổi định dạng file: tài liệu đã lưu vẫn mở được bằng bản cũ (recipe và
phản ứng chỉ là ghi chú thông thường).
