# 图片解码夹具

`go-video-001-q50-420.jpg` 来自 Go 标准库的 `image/testdata/video-001.q50.420.jpeg`
（BSD-3-Clause），是 Go 自己的 JPEG 解码器测试图。这里只用于验证
`ebook_format::image::decode_image` 的 JPEG 分支（4:2:0 色度抽样），
不带任何业务含义。

如果需要换图：保持体积在几 KB，并确认 `image_size` 与 `decode_image`
报同一组宽高。
