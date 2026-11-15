# Bến

Dock cho Windows 11: một hàng icon ứng dụng ở mép dưới màn hình, icon phóng to theo con trỏ, chạy ngầm từ lúc đăng nhập. Viết bằng Rust, hiển thị bằng Windows Composition (bộ ghép hình của chính Windows lo việc vẽ từng khung hình).

## Gồm những gì

| Thành phần | Mô tả |
|---|---|
| Dock | Hàng icon ở mép dưới, phóng to theo con trỏ, kéo để đổi chỗ |
| Thanh trên cùng | Bên trái: dấu mỏ neo của Bến, tên ứng dụng đang dùng (chữ đậm) và các menu Tệp, Sửa, Xem, Cửa sổ, Trợ giúp. Bên phải: âm lượng, pin, mạng, tìm kiếm, ngày giờ |
| Lưới ứng dụng | Toàn bộ ứng dụng đã cài, chia trang, gõ để tìm |
| Hiệu ứng cửa sổ | Thu nhỏ về icon và bay ngược ra, xem trước cửa sổ khi rê chuột |

## Cài và chạy

1. Chạy `setup.bat` một lần. Nó kiểm tra máy đã có C++ Build Tools và Rust chưa, thiếu cái nào thì hỏi trước khi cài.
2. Chạy `build.bat`. File chạy nằm ở `dist\ben.exe` và tự mở sau khi build xong.
3. Chuột phải lên dock, chọn **Khởi động cùng Windows** để Bến tự chạy mỗi lần đăng nhập.

## Dùng

| Thao tác | Kết quả |
|---|---|
| Rê chuột lên dock | Icon phóng to theo con trỏ, hiện tên ứng dụng |
| Bấm icon ứng dụng chưa mở | Mở ứng dụng, icon nảy cho tới khi cửa sổ hiện ra |
| Bấm icon ứng dụng đang mở | Đưa cửa sổ ra trước; bấm lần nữa để thu nhỏ hoặc chuyển sang cửa sổ khác của ứng dụng đó |
| Bấm chuột giữa | Mở thêm một cửa sổ mới |
| Rê chuột và giữ yên trên icon ứng dụng đang mở | Hiện hình thu nhỏ các cửa sổ của ứng dụng đó. Bấm vào một hình để chuyển tới cửa sổ, bấm chuột giữa để đóng nó |
| Thu nhỏ một cửa sổ | Cửa sổ bị hút xuống icon của nó trong dock. Bấm icon để cửa sổ bay ngược ra |
| Giữ chuột trái trên một icon rồi kéo ngang | Đổi chỗ icon, các icon bên cạnh trượt sang nhường chỗ |
| Kéo icon sang ngang | Đổi chỗ icon; icon bên cạnh trượt nhường chỗ |
| Bấm icon đầu tiên (ô vuông 9 chấm) | Mở lưới ứng dụng. Gõ để tìm, Enter mở kết quả đầu, lăn chuột hoặc phím mũi tên để sang trang, Esc hoặc bấm ra ngoài để đóng |
| Bấm dấu mỏ neo trên thanh trên | Menu của Bến và của máy: Cài đặt Bến, Cài đặt Windows, hiện taskbar, khởi động cùng Windows, ngủ, khởi động lại, tắt máy, khoá màn hình, đăng xuất, thoát Bến |
| Bấm tên ứng dụng trên thanh trên | Thu nhỏ, phóng to, đóng cửa sổ, thoát ứng dụng đang ở trước |
| Bấm Tệp, Sửa, Xem, Cửa sổ, Trợ giúp | Mở menu tương ứng. Khi một menu đang mở, rê sang tên khác là chuyển menu; phím mũi tên lên xuống để chọn, Enter để bấm, Esc để đóng |
| Bấm kính lúp trên thanh trên | Mở lưới ứng dụng |
| Bấm cái loa | Thanh kéo âm lượng, tắt tiếng, Cài đặt âm thanh |
| Bấm pin | Phần trăm pin, đang dùng pin hay bộ sạc, thời gian còn lại nếu Windows ước được, Cài đặt pin |
| Bấm biểu tượng mạng | Danh sách mạng Wi-Fi quanh đây kèm độ mạnh sóng và ổ khoá, mạng đang nối có dấu tích. Bấm một mạng để nối; mạng mới có mật khẩu thì gõ mật khẩu ngay trong menu (bấm con mắt để xem chữ đã gõ) rồi Enter |
| Bấm ngày giờ | Bảng lịch tháng, hôm nay được khoanh tròn; mũi tên để lật tháng, bấm tên tháng để về tháng này; dòng dưới cùng mở thông báo của Windows |
| Chuột phải lên icon | Giữ trong Bến / gỡ khỏi Bến, đóng cửa sổ |
| Chuột phải bất kỳ chỗ nào trên dock | Mở cửa sổ Cài đặt, hiện lại taskbar Windows, bật khởi động cùng Windows, thoát |

Các menu Tệp, Sửa, Xem không phải menu thật của từng ứng dụng (Windows không cho lấy menu của ứng dụng khác đưa lên thanh). Chúng chứa những lệnh hầu như ứng dụng nào cũng có, và khi bấm thì Bến gõ hộ phím tắt tương ứng (ghi ở bên phải mỗi dòng) vào ứng dụng đang ở trước. Ứng dụng nào không dùng phím tắt đó thì dòng đó không có tác dụng.

Mật khẩu Wi-Fi gõ trong menu được giao thẳng cho dịch vụ Wi-Fi của Windows để lưu mạng, giống như khi nối từ bảng của Windows. Bến không lưu mật khẩu ở chỗ nào khác và không ghi vào log. Nếu sau 15 giây không nối được, mạng vừa lưu bị xoá lại và menu hỏi lại mật khẩu. Từ Windows 11 24H2, Windows chỉ cho ứng dụng xem danh sách Wi-Fi khi Vị trí đang bật; nếu đang tắt, menu sẽ nói rõ và có dòng mở trang cài đặt Vị trí.

Vạch nhỏ dưới icon cho biết ứng dụng đang mở. Vạch dài hơn và có màu nhấn của Windows là ứng dụng đang ở trước.

Dock tự trượt xuống khi có ứng dụng chiếm toàn màn hình (game, video) và trượt lên lại khi thoát toàn màn hình.

## Hiệu ứng thu nhỏ và Windows

Khi hiệu ứng thu nhỏ về dock đang bật, Bến tạm tắt hiệu ứng thu nhỏ / phóng to có sẵn của Windows, để hai hiệu ứng không chạy chồng lên nhau. Thoát Bến, hoặc tắt hiệu ứng trong Cài đặt, thì hiệu ứng của Windows trở lại.

Cửa sổ mở lại bằng cách khác (Alt+Tab, bấm vào thông báo) sẽ hiện ra ngay, không có hiệu ứng.

## Taskbar của Windows

Khi Bến chạy, taskbar bị ẩn. Thoát Bến thì taskbar trở lại như cũ.

Thanh trên cùng thay cho phần đồng hồ, pin, âm lượng, mạng của taskbar. Các icon nhỏ của ứng dụng chạy nền (khay hệ thống) thì chưa có: muốn xem, chuột phải lên dock hoặc bấm dấu mỏ neo trên thanh trên, chọn **Hiện taskbar của Windows**. Nút Start vẫn mở bằng phím Windows.

Nếu Bến bị tắt đột ngột và taskbar không trở lại: chạy `restore-taskbar.bat`.

## Tùy chỉnh

Có hai cách, cả hai đều có hiệu lực ngay, không cần mở lại dock:

1. **Cửa sổ Cài đặt**: chuột phải lên dock, chọn **Cài đặt…**. Kéo thanh trượt là dock đổi theo trong lúc kéo.
2. **File `ben.ini`** cạnh `ben.exe`: sửa rồi lưu. Mỗi thông số có một dòng giải thích ngay phía trên.

Các nhóm thông số:

| Nhóm | Chỉnh được gì |
|---|---|
| Kích thước | Cỡ icon, khoảng cách giữa icon, lề trong của nền, độ bo góc, khoảng cách tới mép dưới màn hình |
| Phóng to | Độ phóng to, độ lan sang icon bên cạnh, tốc độ phồng lên / xẹp xuống, nền có cao lên theo icon hay không |
| Giao diện | Màu nền (theo Windows / tối / sáng), độ đục, nền mờ kính và độ mờ, vạch báo app đang mở |
| Thanh trên và lưới ứng dụng | Bật tắt thanh trên và chiều cao, bật tắt nút lưới ứng dụng, cỡ icon trong lưới |
| Chuyển động | Tốc độ chung, hiện tên khi rê chuột và cỡ chữ, icon nảy (bật tắt, độ cao, thời gian) |
| Hành vi | Bấm icon để thu nhỏ, ẩn dock khi có app toàn màn hình, ẩn taskbar Windows, giữ chỗ cho dock |
| Cửa sổ | Hiệu ứng thu nhỏ về dock (tắt / co lại / hút / trượt) và thời gian, xem trước cửa sổ khi rê chuột (bật tắt, thời gian chờ, cỡ hình) |

Các file khác cạnh `ben.exe`:

| File | Nội dung |
|---|---|
| `pins.txt` | Danh sách ứng dụng ghim, mỗi dòng một đường dẫn `.exe` |
| `ben.log` | Nhật ký chạy, dùng khi cần tìm lỗi |

## Dòng lệnh

```
ben.exe                     mở dock
ben.exe --quit              tắt dock đang chạy
ben.exe --restore-taskbar   đưa taskbar Windows trở lại
ben.exe --autostart on|off  bật / tắt khởi động cùng Windows
ben.exe --bench             mở dock rồi tự chạy thử 9 giây, ghi tốc độ khung hình vào ben.log
```

## Mã nguồn

| File | Việc |
|---|---|
| `src/main.rs` | Tạo cửa sổ, vòng lặp khung hình |
| `src/dock.rs` | Danh sách icon, chuyển động, vẽ, xử lý bấm chuột |
| `src/layout.rs` | Công thức phóng to icon theo con trỏ |
| `src/spring.rs` | Chuyển động lò xo, không phụ thuộc tần số màn hình |
| `src/warp.rs` | Hình học của hiệu ứng cửa sổ bay vào icon |
| `src/fx.rs` | Phát hiệu ứng thu nhỏ / mở lại: chụp hình cửa sổ một lần rồi uốn tấm hình đó |
| `src/preview.rs` | Bảng xem trước cửa sổ khi rê chuột |
| `src/bar.rs` | Thanh trên cùng |
| `src/menu.rs` | Menu thả xuống do Bến tự vẽ |
| `src/menus.rs` | Nội dung từng menu |
| `src/calendar.rs` | Bảng lịch dưới đồng hồ |
| `src/dates.rs` | Phép tính ngày tháng cho bảng lịch |
| `src/wifi.rs` | Danh sách mạng Wi-Fi và nối mạng, qua dịch vụ Wi-Fi của Windows |
| `src/launcher.rs` | Lưới ứng dụng toàn màn hình |
| `src/text.rs` | Vẽ chữ một lần rồi dùng lại |
| `src/gfx.rs` | Thiết bị đồ họa (Direct3D, Direct2D, DirectWrite) |
| `src/glass.rs` | Cây hình ảnh của dock (Windows Composition): nền mờ kính, icon, vạch báo, tên app |
| `src/shell.rs` | Cửa sổ đang mở, icon, tên, mở và chuyển ứng dụng |
| `src/system.rs` | Màu hệ thống, khởi động cùng Windows, ẩn và trả taskbar |
| `src/config.rs` | Bảng các thông số, đọc và ghi `ben.ini`, `pins.txt` |
| `src/settings.rs` | Cửa sổ Cài đặt |
