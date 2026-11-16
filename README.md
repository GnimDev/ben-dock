# Bến

Giao diện "chất lỏng" cho Windows 11: một dock bằng kính ở mép dưới và một đảo động ở mép trên của mỗi màn hình, thay cho taskbar. Mọi thứ chuyển động bằng lò xo. Viết bằng Rust, hiển thị bằng Windows Composition (bộ ghép hình của chính Windows vẽ từng khung hình), chạy ngầm từ lúc đăng nhập.

Nghiên cứu, số đo và lý do của từng quyết định thiết kế: [docs/nghien-cuu-fluid.md](docs/nghien-cuu-fluid.md).

## Gồm những gì

| Thành phần | Mô tả |
|---|---|
| Dock | Viên kính chứa icon ứng dụng. Icon phồng lên dưới con trỏ, mép kính co giãn theo, một giọt sáng trượt theo chuột, icon lún xuống khi nhấn |
| Đảo động | Viên kính nhỏ ở giữa mép trên: giờ, ngày, mạng, pin. Nở ra báo khi đổi âm lượng, cắm / rút sạc, pin yếu, có / mất mạng, đổi bài hát. Rê chuột vào thì mở thành bảng điều khiển |
| Trang con của đảo | Wi-Fi, Bluetooth, Âm thanh, Pin, Lịch: Bến tự vẽ, không mở bảng của Windows |
| Menu | Menu chuột phải bằng kính, mọc ra từ icon |
| Khay ứng dụng | Toàn bộ ứng dụng đã cài, chia trang, gõ để tìm |
| Xem trước cửa sổ | Rê chuột lên icon đang mở: hình sống của các cửa sổ |
| Cửa sổ nổi | Cửa sổ vừa mở không phóng to hết màn hình mà nổi ở giữa, chiếm một phần màn hình (bật sẵn) |
| Hiệu ứng cửa sổ | Thu nhỏ bằng cách nào cửa sổ cũng bay về đúng icon của nó; bấm icon thì cửa sổ bị hút vào icon qua một cái phễu cong (kiểu thần đèn trên Mac) và chảy ngược ra khi mở lại |
| Chủ đề | Bốn chất liệu (kính lỏng, sương mờ, mực, đặc), sáng / tối / theo Windows, nhuộm màu nhấn |

Có hai màn hình trở lên thì mỗi màn hình có dock và đảo riêng. Dock của màn hình nào hiện ứng dụng ghim cùng các ứng dụng có cửa sổ trên màn hình đó.

## Cài và chạy

1. Chạy `setup.bat` một lần. Nó kiểm tra máy đã có Rust và bộ liên kết chưa, thiếu thì hỏi trước khi cài.
2. Chạy `build.bat`. File chạy nằm ở `dist\ben.exe` và tự mở sau khi build xong.
3. Chuột phải lên dock, chọn **Khởi động cùng Windows** để Bến tự chạy mỗi lần đăng nhập.

Rust dùng được cả hai bản: MSVC (cần C++ Build Tools) hoặc GNU (cần `dlltool` và `as` của MinGW trên PATH, ví dụ `C:\msys64\ucrt64\bin`).

## Dùng

### Dock

| Thao tác | Kết quả |
|---|---|
| Rê chuột lên dock | Icon phồng lên theo con trỏ, hiện tên ứng dụng |
| Bấm icon ứng dụng chưa mở | Mở ứng dụng, icon nảy cho tới khi cửa sổ hiện ra |
| Bấm icon ứng dụng đang mở | Đưa cửa sổ ra trước; đang ở trước thì thu nhỏ, hoặc chuyển sang cửa sổ khác của ứng dụng đó |
| Bấm icon có cửa sổ đang thu nhỏ | Cửa sổ chảy từ icon ra chỗ cũ |
| Bấm chuột giữa | Mở thêm một cửa sổ mới |
| Giữ yên chuột trên icon đang mở | Hình sống các cửa sổ. Bấm một hình để chuyển tới, bấm chuột giữa để đóng |
| Giữ chuột trái trên icon rồi kéo ngang | Đổi chỗ icon; icon bên cạnh trượt nhường chỗ |
| Bấm icon đầu tiên (9 chấm) | Mở khay ứng dụng. Gõ để tìm, Enter mở kết quả đầu, lăn chuột hoặc phím mũi tên để sang trang, Esc để đóng |
| Chuột phải lên icon | Mở cửa sổ mới, giữ trong dock / gỡ khỏi dock, đóng cửa sổ |
| Chuột phải lên dock | Cài đặt Bến, hiện taskbar của Windows, khởi động cùng Windows, thoát |

Chấm nhỏ dưới icon cho biết ứng dụng đang mở. Chấm dài ra và mang màu nhấn của Windows là ứng dụng đang ở trước. Dock và đảo trượt khỏi màn hình khi có ứng dụng chiếm toàn màn hình đó.

### Đảo động

| Thao tác | Kết quả |
|---|---|
| Rê chuột vào đảo (hoặc bấm) | Mở bảng: giờ và ngày, bài đang phát và nút điều khiển, thanh âm lượng, bốn thẻ, bốn nút |
| Kéo thanh âm lượng, hoặc lăn chuột trên đảo | Đổi âm lượng |
| Thẻ **Wi-Fi** | Mạng quanh đây, độ mạnh sóng, ổ khoá; mạng đang nối có dấu tích. Bấm để nối; mạng mới có mật khẩu thì gõ ngay trong trang rồi Enter |
| Thẻ **Bluetooth** | Công tắc bật / tắt, thiết bị đã ghép đôi, thiết bị nào đang nối |
| Thẻ **Âm thanh** | Tắt tiếng, chọn loa / tai nghe để phát |
| Thẻ **Pin** | Phần trăm, đang sạc hay dùng pin, thời gian còn lại nếu Windows ước được |
| Bấm vào ngày | Lịch tháng; mũi tên để lật tháng |
| Mũi tên ở góc trái của trang | Về bảng chính |
| **Cài đặt**, **Khoá**, **Ngủ**, **Nguồn** | Cửa sổ Cài đặt Bến, khoá màn hình, ngủ, menu khởi động lại / tắt máy / đăng xuất |

Mỗi trang có một dòng mở trang cài đặt tương ứng của Windows cho những việc Bến chưa làm.

Mật khẩu Wi-Fi gõ trong trang được giao thẳng cho dịch vụ Wi-Fi của Windows để lưu mạng, giống như khi nối từ bảng của Windows. Bến không lưu mật khẩu ở chỗ nào khác và không ghi vào log. Nếu sau 15 giây không nối được, mạng vừa lưu bị xoá lại và trang hỏi lại mật khẩu. Từ Windows 11 24H2, Windows chỉ cho ứng dụng xem danh sách Wi-Fi khi Vị trí đang bật; nếu đang tắt, trang sẽ nói rõ và có dòng mở cài đặt Vị trí.

## Cửa sổ mới mở dạng nổi

Mặc định, cửa sổ vừa mở không chiếm hết màn hình: Bến đưa nó về dạng cửa sổ nổi, nằm giữa màn hình nó mở ra, rộng và cao nhiều nhất 70% màn hình đó (chỉnh được trong Cài đặt, nhóm **Cửa sổ**).

| Cửa sổ mở ra | Bến làm gì |
|---|---|
| Phóng to hết màn hình | Thôi phóng to, thu về đúng cỡ nổi, đặt giữa |
| Không phóng to nhưng to hơn cỡ nổi | Cắt bớt chiều nào quá cỡ, đặt giữa |
| Nhỏ hơn cỡ nổi | Giữ nguyên, kể cả vị trí |
| Không đổi cỡ được, đang thu nhỏ, trò chơi hay video toàn màn hình | Giữ nguyên |
| Tự phóng to một nhịp sau khi hiện (nhiều app làm vậy) | Vẫn được đưa về dạng nổi, trong 1,5 giây đầu |

Bến chỉ làm việc này một lần, lúc cửa sổ mở ra. Sau đó bạn phóng to hay kéo cỡ thế nào thì nó ở yên như vậy; chỉ cần bạn bắt đầu kéo cửa sổ là Bến thôi để ý tới nó. Cửa sổ đã mở sẵn từ trước khi Bến chạy, và cửa sổ ở desktop ảo khác, không bị đụng tới. Nhiều cửa sổ mở cùng lúc thì xếp lệch nhau một chút thay vì chồng khít lên nhau.

## Thu nhỏ và mở lại cửa sổ

| Cách thu nhỏ / mở lại | Chuyện gì xảy ra |
|---|---|
| Nút trên cửa sổ, bàn phím, Alt+Tab, Win+D | Hoạt ảnh của Windows, nhưng bay về (và bay ra từ) đúng icon của cửa sổ trên dock của màn hình nó đang ở. Icon phồng ra "nuốt" cửa sổ |
| Bấm icon trên dock | Hiệu ứng của Bến, chọn trong Cài đặt: co lại, chảy, trượt; hoặc để Windows diễn |

Kiểu **chảy** là hiệu ứng thần đèn: chân cửa sổ vươn xuống icon và thắt lại, hai mép uốn cong thành phễu, rồi cả cửa sổ bị hút qua phễu vào icon, đỉnh đi sau cùng. Mở lại thì diễn ngược. Để uốn được, Bến chụp một ảnh của cửa sổ (ngoài màn hình, ở luồng riêng, kể cả khi cửa sổ đang thu nhỏ) rồi cắt ảnh thành khoảng 300 dải ngang; trong vài phần trăm giây chờ ảnh, cửa sổ đứng yên tại chỗ. Thời gian mặc định 0,36 giây; trên Mac hiệu ứng này dài khoảng 0,45 giây (chỉnh ở **Thời gian hiệu ứng thu nhỏ**). Cửa sổ nào không cho chụp (nội dung được bảo vệ) thì bay nguyên khối, không uốn cong.

Bến không đổi cài đặt hoạt ảnh nào của Windows. Trong lúc hiệu ứng của Bến diễn cho một cửa sổ, hoạt ảnh của Windows được tắt cho riêng cửa sổ đó rồi bật lại ngay sau.

## Taskbar của Windows

Khi Bến chạy, taskbar bị ẩn. Thoát Bến thì taskbar trở lại như cũ. Các icon nhỏ của ứng dụng chạy nền (khay hệ thống) thì Bến chưa có: muốn xem, chuột phải lên dock, chọn **Hiện taskbar của Windows**. Nút Start vẫn mở bằng phím Windows.

Nếu Bến bị tắt đột ngột và taskbar không trở lại: chạy `restore-taskbar.bat`.

## Tùy chỉnh

Có hai cách, cả hai đều có hiệu lực ngay, không cần mở lại Bến:

1. **Cửa sổ Cài đặt**: chuột phải lên dock, chọn **Cài đặt Bến…**. Kéo thanh trượt là giao diện đổi theo trong lúc kéo.
2. **File `ben.ini`** cạnh `ben.exe`: sửa rồi lưu. Mỗi thông số có một dòng giải thích ngay phía trên.

| Nhóm | Chỉnh được gì |
|---|---|
| Giao diện | Chất liệu, sáng / tối, độ đục của kính, độ nhoè phía sau, nhuộm màu nhấn, đổ bóng |
| Chuyển động | Tốc độ chung, độ đàn hồi (0 = dừng êm, 1 = nảy nhiều nhất) |
| Đảo động | Bật tắt, chiều cao, rê chuột để mở, thông báo nhanh, nhạc đang phát |
| Dock | Cỡ icon, khoảng cách, lề, bo góc, độ phóng to và độ lan, giọt sáng, chấm báo, tên ứng dụng, icon nảy |
| Hành vi | Bấm icon để thu nhỏ, ẩn khi có app toàn màn hình, ẩn taskbar, giữ chỗ cho dock và đảo, khay ứng dụng |
| Cửa sổ | Cửa sổ mới mở dạng nổi và cỡ của nó, hiệu ứng thu nhỏ và thời gian, xem trước cửa sổ |
| Chẩn đoán (chỉ trong `ben.ini`) | Ghi số đo vào log, vẽ bằng card đồ hoạ, giới hạn RAM |

Các file khác cạnh `ben.exe`:

| File | Nội dung |
|---|---|
| `pins.txt` | Danh sách ứng dụng ghim, mỗi dòng một đường dẫn |
| `ben.log` | Nhật ký chạy, dùng khi cần tìm lỗi |

## Dòng lệnh

```
ben.exe                     mở Bến
ben.exe --quit              tắt Bến đang chạy
ben.exe --restore-taskbar   đưa taskbar Windows trở lại
ben.exe --autostart on|off  bật / tắt khởi động cùng Windows
ben.exe --demo              mở Bến rồi lần lượt trình diễn mọi bề mặt (khoảng 36 giây)
ben.exe --bench             mở Bến rồi tự chạy thử, ghi tốc độ khung hình vào ben.log
```

## Mã nguồn

| File | Việc |
|---|---|
| `src/main.rs` | Tạo cửa sổ, vòng lặp khung hình, đồng hồ khung hình |
| `src/app.rs` | Điều phối: các màn hình, sự kiện, một cú bấm làm gì, các trang của đảo |
| `src/scene.rs` | Chất liệu kính và các hình cơ bản trên Windows Composition |
| `src/shade.rs` | Cửa sổ chỉ chứa bóng đổ |
| `src/motion.rs`, `src/spring.rs` | Các kiểu lò xo, chuyển động không phụ thuộc tần số màn hình |
| `src/theme.rs` | Chủ đề: chất liệu, màu chữ, màu nhấn |
| `src/dock.rs`, `src/layout.rs` | Dock và công thức phóng to icon |
| `src/island.rs` | Đảo động và các trang của nó |
| `src/bubble.rs` | Menu bằng kính |
| `src/launcher.rs` | Khay ứng dụng |
| `src/preview.rs` | Xem trước cửa sổ |
| `src/fx.rs`, `src/warp.rs` | Hiệu ứng cửa sổ chảy vào icon và hình học của nó |
| `src/camera.rs` | Chụp ảnh một cửa sổ ngoài màn hình, ở luồng riêng |
| `src/media.rs` | Bài đang phát |
| `src/wifi.rs`, `src/audio.rs`, `src/bluetooth.rs` | Wi-Fi, thiết bị phát, Bluetooth |
| `src/dates.rs` | Phép tính ngày tháng cho lịch |
| `src/shell.rs` | Cửa sổ đang mở, icon, tên, mở và chuyển ứng dụng |
| `src/system.rs` | Màn hình, màu hệ thống, pin, âm lượng, taskbar, bộ nhớ |
| `src/text.rs`, `src/gfx.rs` | Chữ và thiết bị đồ hoạ |
| `src/config.rs`, `src/settings.rs` | Thông số, `ben.ini`, cửa sổ Cài đặt |
