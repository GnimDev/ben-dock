# Bến 0.2: giao diện "chất lỏng" cho Windows

Tài liệu này ghi lại kế hoạch nghiên cứu, những gì đã thử trên máy thật, thiết kế rút ra từ đó và phần còn để ngỏ. Mọi con số đều đo trên máy phát triển: Windows 11 bản 26200, hai màn hình 1920×1080 (màn hình chính 165 Hz tỉ lệ 100%, màn hình phụ tỉ lệ 125%).

## 1. Mục tiêu

- Mọi thứ chuyển động, ngắn gọn nhưng mượt, cảm giác nhẹ và tự nhiên như chất lỏng.
- Bản đầu phải chạy được: chủ đề (theme), đảo động, dock đàn hồi, các hiệu ứng mở / đóng.
- Viết bằng Rust, tối ưu tối đa; chạy nền dưới 300 MB RAM.
- Thiết kế lại từ đầu, không giữ giao diện kiểu macOS của bản 0.1.

## 2. Kế hoạch nghiên cứu

| Câu hỏi | Cách trả lời |
|---|---|
| Phần nào của Windows nhận được hiệu ứng, bằng API nào? | Đọc tài liệu, rồi thử từng API bằng chương trình nhỏ |
| Chuyển động thế nào thì "lỏng" mà không lố? | Tổng hợp thông số lò xo từ các hệ thống chuyển động đã công bố |
| Vẽ bằng gì để nhẹ máy nhất? | Đo tốc độ khung hình và RAM của từng phương án |
| Cái gì không làm được? | Ghi rõ để không hứa quá |

## 3. Windows cho phép làm gì

| Thành phần | Làm được | Bằng gì | Ghi chú |
|---|---|---|---|
| Dock, đảo, menu, khay ứng dụng | Có | Cửa sổ trong suốt + Windows.UI.Composition | Bộ ghép hình của Windows vẽ từng khung hình; app chỉ gửi thay đổi |
| Kính mờ nhìn xuyên xuống nền | Có | `CreateBackdropBrush` + nhoè + tăng bão hoà | `CreateHostBackdropBrush` cho ra kính đen trên bản 26200 |
| Khúc xạ thật (bẻ cong ảnh nền) | Không | - | Composition không có shader tự viết; dùng viền sáng và dải bóng để gợi cảm giác kính |
| Bóng đổ ra ngoài mặt kính | Có | Cửa sổ riêng chỉ chứa bóng, cho chuột xuyên qua | `SetWindowRgn` cắt luôn phần bóng nếu vẽ chung cửa sổ |
| Hiệu ứng thu nhỏ / mở lại cửa sổ của app khác | Có | Xem mục 6 | Không thay được hoạt ảnh mở / đóng cửa sổ của Windows |
| Thông báo (toast) của app khác trên đảo | Chưa | - | Cần app có danh tính gói (package identity) |
| Bài đang phát, điều khiển nhạc | Có | `GlobalSystemMediaTransportControlsSession` | Chạy ở luồng riêng |
| Wi-Fi, âm lượng, thiết bị phát, Bluetooth, pin | Có | WLAN API, Core Audio, `Windows.Devices.Radios`, trạng thái nguồn | Đổi thiết bị phát mặc định phải dùng `IPolicyConfig` (không có trong tài liệu chính thức; EarTrumpet và SoundSwitch cũng dùng) |
| Đổi viền, góc bo, nền tối của cửa sổ app khác | Có | `DwmSetWindowAttribute` gọi từ tiến trình khác | Chưa dùng trong bản này |

## 4. Ngôn ngữ chuyển động

Mọi chuyển động là lò xo giảm chấn, tính bằng nghiệm chính xác nên giống nhau ở 60 hay 165 Hz. Một lò xo được cảm nhận dài bằng một chu kỳ, 2π/ω.

| Loại | ω (rad/s) | Độ nảy cho phép | Dùng cho |
|---|---|---|---|
| tiny | 56 | 0,20 | Nhấn, nhả |
| small | 38 | 0,30 | Phóng to icon, chấm báo |
| medium | 24 | 0,56 | Bề mặt mở ra, mép kính của dock |
| large | 18 | 0,26 | Đảo mở thành bảng |
| fade | 40 | 0 | Độ trong và màu: không bao giờ nảy |
| calm | 26 | 0 | Bề mặt đóng lại |
| follow | 44 | 0,44 | Thứ bám theo con trỏ |
| jelly | 30 | 0,90 | Icon "nuốt" cửa sổ |

Hệ số giảm chấn = 1 − độ nảy × thông số `elastic`, không xuống dưới 0,5 (dưới mức đó chuyển động trông như đồ chơi). Quy tắc đi kèm:

- Phản ứng ngay lúc nhấn xuống, không đợi nhả.
- Bề mặt mới mọc ra từ thứ vừa được bấm (menu từ icon, trang con từ thẻ).
- Vật chuyển động kéo dài theo hướng đi và dẹt lại khi chạm đích.
- Bo góc đồng tâm: góc trong = góc ngoài − lề.
- Khi Windows tắt "Hiệu ứng hoạt ảnh", mọi lò xo thành giảm chấn tới hạn và nhanh hơn 1,6 lần.

## 5. Kiến trúc

| Phần | File | Việc |
|---|---|---|
| Vòng lặp | `main.rs` | Ngủ khi không có gì chuyển động; khi có thì một lượt mỗi khung hình |
| Điều phối | `app.rs` | Giữ mọi bề mặt, nghe sự kiện, quyết định một cú bấm làm gì |
| Cảnh | `scene.rs`, `shade.rs` | Chất liệu kính, bóng, các hình cơ bản |
| Chuyển động | `motion.rs`, `spring.rs` | Lò xo và các "giọng" ở mục 4 |
| Bề mặt | `dock.rs`, `island.rs`, `bubble.rs`, `launcher.rs`, `preview.rs` | Mỗi bề mặt chỉ biết tự vẽ và tự chuyển động |
| Hiệu ứng cửa sổ | `fx.rs`, `warp.rs`, `camera.rs` | Mục 6 |
| Hệ thống | `shell.rs`, `system.rs`, `media.rs`, `wifi.rs`, `audio.rs`, `bluetooth.rs` | Nói chuyện với Windows |

**Nhiều màn hình.** Mỗi màn hình có một dock và một đảo riêng, vẽ theo tỉ lệ của màn hình đó. Dock của màn hình nào hiện app ghim cùng các app có cửa sổ trên màn hình ấy. Menu, khay ứng dụng, bảng xem trước và hiệu ứng cửa sổ chỉ có một bộ, mở trên màn hình vừa gọi chúng. Cửa sổ đang thu nhỏ ở lại với dock của màn hình cuối cùng nó hiện trên đó.

**Đảo và các trang con.** Đảo thu gọn là đồng hồ, ngày, mạng, pin. Mở ra là bảng điều khiển: âm lượng, bốn thẻ, bốn nút. Mỗi thẻ biến bảng thành trang riêng:

| Trang | Nội dung |
|---|---|
| Wi-Fi | Mạng quanh đây, độ mạnh sóng, ổ khoá; bấm để nối, gõ mật khẩu ngay trong trang |
| Bluetooth | Công tắc bật / tắt, thiết bị đã ghép đôi và thiết bị đang nối |
| Âm thanh | Tắt tiếng, chọn thiết bị phát |
| Pin | Phần trăm, đang sạc hay dùng pin, thời gian còn lại |
| Lịch | Bấm vào ngày: lịch tháng, lật tháng |

## 6. Thu nhỏ và mở lại cửa sổ

Đây là phần được làm lại nhiều nhất, vì bản trước "lúc có lúc không" và hay khựng. Ba nguyên nhân tìm được, đều kiểm chứng bằng chương trình thử:

1. **Vòng lặp bị điếc trong lúc chờ khung hình.** Khi một cửa sổ sắp thu nhỏ, Windows hỏi các shell "icon của cửa sổ này ở đâu?" và chỉ đợi 100 ms. Bộ ghép hình chưa xong khung hình chừng nào câu hỏi đó chưa xong. Nếu lúc ấy Bến đang chờ đúng khung hình đó bằng `DwmFlush`, hai bên chờ nhau: đo được 125 đến 137 ms đứng hình mỗi lần, rồi Windows bỏ qua Bến. Cách sửa: một luồng phụ chờ bộ ghép hình, luồng chính chờ luồng phụ theo cách vẫn trả lời được câu hỏi.
2. **Tắt hoạt ảnh của Windows trên toàn hệ thống.** Bản trước tắt hẳn hoạt ảnh thu nhỏ / phóng to của Windows để tự diễn. Hệ quả: cửa sổ mở lại bằng Alt+Tab hiện ra cụt lủn, phóng to cũng mất hoạt ảnh, và nếu Bến bị tắt đột ngột thì máy ở luôn trong trạng thái đó. Bản này không đụng tới cài đặt ấy nữa.
3. **Chụp màn hình trước khi diễn, ngay trên luồng chính.** Hiệu ứng cũ dựng ảnh cửa sổ lên màn hình, chụp lại phần màn hình đó rồi cắt thành dải; trong lúc chụp và nạp ảnh, mọi thứ của Bến đứng hình vài chục ms.

Thiết kế mới có hai lớp:

| Lớp | Khi nào | Cách làm |
|---|---|---|
| Hoạt ảnh của Windows, bay về đúng icon | Mọi cách thu nhỏ / mở lại: nút trên cửa sổ, bàn phím, Alt+Tab, Win+D | Bến trả lời câu hỏi "icon ở đâu" bằng vị trí icon trên dock của màn hình chứa cửa sổ. Windows tự diễn, Bến không tốn gì. Icon phồng ra "nuốt" cửa sổ rồi rung nhẹ |
| Hiệu ứng riêng của Bến (co lại / chảy / trượt) | Khi bấm icon trên dock | Xem bên dưới. Trong lúc diễn, Bến tắt hoạt ảnh của Windows cho riêng cửa sổ đó (`DWMWA_TRANSITIONS_FORCEDISABLED`, gọi được từ tiến trình khác) để hai bên không chồng lên nhau |

**Kiểu chảy là hiệu ứng thần đèn như trên Mac:** chân cửa sổ vươn xuống icon và thắt lại, hai mép cong thành phễu, rồi cả cửa sổ bị hút qua phễu, đỉnh đi sau cùng. Muốn bẻ cong hai mép thì phải cắt hình cửa sổ thành nhiều dải ngang, mỗi dải ép về bề rộng của phễu ở độ cao đó. Có ba cách lấy hình để cắt, đo với hiệu ứng dài 300 ms trên màn hình 165 Hz (tối đa 50 khung hình):

| Phương án | Số khung hình trong 300 ms | Kết luận |
|---|---|---|
| Một ảnh sống (DWM thumbnail) bay nguyên khối | 44 đến 50 | Dùng cho kiểu co lại và trượt: không cần cắt |
| 90 ảnh sống, mỗi dải một ảnh | 15 đến 17 | Bỏ: Windows phải vẽ lại cửa sổ 90 lần mỗi khung hình |
| Một ảnh chụp, cắt thành khoảng 300 dải là hình ghép bình thường | 43 đến 49 | Dùng cho kiểu chảy: phần việc của Bến dưới 1,2 ms mỗi khung |

Ảnh chụp là thứ từng gây khựng, nên cách chụp được làm lại (`camera.rs`):

| Vấn đề | Cách giải |
|---|---|
| Chụp không được che màn hình, và phải chụp được cả cửa sổ đang thu nhỏ | Một cửa sổ phụ bị "che áo" (`DWMWA_CLOAK`: vẫn được ghép hình nhưng không bao giờ lên màn hình) nhận ảnh sống của cửa sổ cần chụp, rồi được in ra bitmap bằng `PrintWindow` kèm cờ vẽ đủ nội dung. Thử thấy cách này chụp được cả cửa sổ đang thu nhỏ, thứ mà in thẳng không làm được |
| Chụp mất 20 đến 40 ms với cửa sổ lớn | Chạy ở luồng riêng. Trong lúc đó ảnh sống đứng thế chỗ cửa sổ (khi thu nhỏ), nên không thấy gì khác lạ; hiệu ứng bắt đầu khi ảnh tới |
| Ảnh in ra không có độ trong suốt, bốn góc bo thành màu đen | Cửa sổ phụ nền trắng, bên dưới có một dải đen bày lại bốn góc. Điểm ảnh nào trên nền trắng sáng hơn trên nền đen thì trong suốt đúng bằng độ chênh đó. Một lần in cho cả hai |
| Nạp ảnh cho bộ ghép hình | Cỡ đầy đủ nạp ngay (khoảng 1 ms), năm bản thu nhỏ dần nạp mỗi khung một bản |
| Chuyển từ ảnh sống sang ảnh chụp không nháy | Các dải hiện ra bên dưới ảnh sống, ba khung hình sau ảnh sống mới được gỡ, rồi đồng hồ của hiệu ứng mới chạy |

Không giữ lại ảnh nào sau hiệu ứng: lúc mở lại, cửa sổ đang thu nhỏ được chụp mới. Nếu không chụp được (nội dung được bảo vệ, hoặc quá 250 ms), ảnh sống bay nguyên khối theo đúng đường viền của phễu.

Hai chi tiết đáng nhớ về câu hỏi của Windows (`HSHELL_GETMINRECT`): thứ đến tay cửa sổ shell là cửa sổ kèm bốn số 16 bit (trái, trên, phải, dưới), không phải cấu trúc như tài liệu ghi; và Explorer được hỏi trước, câu trả lời sau cùng mới là câu Windows dùng.

## 7. Cửa sổ mới mở dạng nổi

Yêu cầu: mặc định, một cửa sổ vừa mở không chiếm hết màn hình mà chỉ nổi ở giữa, chiếm một phần màn hình. Windows không có chỗ nào để đặt quy tắc này cho mọi app, và không can thiệp được trước khi cửa sổ hiện ra (muốn vậy phải cấy mã vào từng app). Nên Bến làm ngay sau khi cửa sổ hiện:

| Câu hỏi | Cách giải |
|---|---|
| Cửa sổ nào là "vừa mở"? | Cửa sổ ứng dụng Bến chưa từng thấy kể từ lúc chạy. Cửa sổ có sẵn lúc Bến khởi động và cửa sổ ở desktop ảo khác được ghi nhận là "đã biết", không bao giờ bị đụng |
| Đưa về cỡ nào? | Nhiều nhất `float_size` (mặc định 70%) chiều rộng và chiều cao vùng làm việc của màn hình chứa nó, đặt giữa. Cửa sổ vốn nhỏ hơn thì giữ nguyên |
| Làm sao cho mượt? | Đặt lại "chỗ bình thường" của cửa sổ rồi cho nó về trạng thái bình thường bằng một lệnh (`SetWindowPlacement`): Windows tự diễn hoạt ảnh từ phóng to về cỡ nổi |
| App tự phóng to sau khi hiện (Electron) | Theo dõi cửa sổ mới trong 1,5 giây, mỗi 120 ms nhìn một lần |
| Người dùng tự phóng to hay kéo cỡ | Hết 1,5 giây, hoặc ngay khi người dùng bắt đầu kéo cửa sổ, Bến thôi theo dõi |

Hai điều rút ra khi thử:

- Chỉ đụng vào cửa sổ đã đứng yên ít nhất 100 ms. Đổi chỗ đúng lúc app đang giữa chừng việc hiện hay phóng to cửa sổ thì hai bên giẫm lên nhau.
- "Chỗ bình thường" mà Windows lưu cho cửa sổ không tính bằng toạ độ màn hình mà tính từ góc vùng làm việc của chính màn hình chứa nó. Tính nhầm theo màn hình chính thì cửa sổ trên màn hình phụ lệch đúng bằng độ chênh giữa hai dải giữ chỗ (ở đây 11 px).

Lệnh đổi chỗ được gọi từ một luồng riêng, vì nó phải chờ app của cửa sổ trả lời, mà app vừa khởi động thường còn bận. Cùng lý do đó, mở ứng dụng (`ShellExecuteEx`) cũng chuyển sang luồng riêng: trước đây mỗi lần bấm mở app, dock đứng hình tới một giây.

## 8. Hiệu năng và bộ nhớ

| Việc | Kết quả |
|---|---|
| Tốc độ khi đang chuyển động | 165 khung hình / giây, phần việc của Bến 0,06 đến 0,14 ms mỗi khung |
| Khi đứng yên | Vòng lặp ngủ; thức khoảng một lần mỗi giây cho đồng hồ |
| RAM sau khi mở mọi bề mặt trên hai màn hình | 85 MB trong RAM, 31 MB riêng |
| RAM khi nghỉ từ 20 giây | Trả lại gần hết: khoảng 5 MB trong RAM, 13 MB riêng |
| Giới hạn cứng | `ram_limit` (mặc định 256 MB), Windows không cho giữ nhiều hơn |

Những quyết định tạo ra các con số đó:

- Vẽ icon và chữ bằng bộ xử lý (WARP) thay vì card đồ hoạ: không phải nạp trình điều khiển đồ hoạ vào tiến trình, bớt khoảng 50 MB.
- Chỉ gửi thay đổi cho bộ ghép hình khi có thứ chuyển động hoặc vừa có sự kiện. Gửi mỗi lượt thì chính lời đáp của bộ ghép hình giữ vòng lặp quay mãi.
- Những thứ lặp lại (thanh nhạc trên đảo) giao cho bộ ghép hình tự lặp, app không thức dậy.
- Không giữ ảnh cửa sổ nào sau hiệu ứng: ảnh chụp cho kiểu chảy được bỏ ngay khi diễn xong, lần sau chụp mới.

## 9. Còn để ngỏ

| Việc | Vì sao chưa làm |
|---|---|
| Danh sách app không cho nổi | Trò chơi chạy dạng cửa sổ phóng to (không phải toàn màn hình thật) hiện vẫn bị đưa về dạng nổi; cần chỗ để người dùng loại trừ từng app |
| Thông báo của app khác trên đảo | Cần đóng gói app để có danh tính gói |
| Icon khay hệ thống | Phải thay luôn vai trò của Explorer với các app chạy nền |
| Nối mạng Wi-Fi doanh nghiệp (đăng nhập bằng tài khoản) | Cần giao diện nhập chứng thực riêng |
| Kết nối / ngắt một thiết bị Bluetooth cụ thể | Windows không có API công khai cho việc này |
| Âm lượng từng ứng dụng | Làm được bằng Core Audio, chưa có trang |
| Đổi viền và góc bo của mọi cửa sổ theo chủ đề | API đã thử được, chưa đưa vào |
| Bàn phím cho dock và đảo | Hiện chỉ menu, khay và ô mật khẩu nhận phím |
