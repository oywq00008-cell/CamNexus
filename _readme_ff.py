"""README 两处修正：

1. "自动下载并校验" —— 代码里只做了可用性校验（跑 -version），没有哈希校验。
   文档不能这么写。
2. 补 FFmpeg 的许可归属。
"""

import io


def patch(path, subs):
    t = open(path, encoding="utf-8").read()
    for old, new in subs:
        assert old in t, f"{path} 未找到：{old[:40]}"
        assert t.count(old) == 1, f"{path} 多处命中：{old[:40]}"
        t = t.replace(old, new, 1)
    open(path, "w", encoding="utf-8").write(t)
    print(path, "已更新")


# ---------------- 中文 ----------------
patch("README.md", [
    (
        "**FFmpeg 不需要自己装**——首次启动会自动下载并校验，存放在 `%LOCALAPPDATA%\\CamNexus`。",
        "**FFmpeg 不需要自己装**——首次启动会自动下载，并在下载后确认它能正常运行（跑一次 `-version`），存放在 `%LOCALAPPDATA%\\CamNexus`。\n\n"
        "> FFmpeg 由本项目以**独立进程**方式调用，不链接、不打包、不随本程序分发；"
        "它由第三方以 GPLv3 发布，版权归 FFmpeg 作者所有，源码见 [ffmpeg.org](https://ffmpeg.org)。",
    ),
    (
        "程序本身不包含也不分发 FFmpeg，由用户机器首次运行时自行下载。",
        "程序本身不包含也不分发 FFmpeg，由用户机器首次运行时自行下载。\n\n"
        "本程序通过命令行调用 FFmpeg（独立进程，不链接其库），使用的是 GPLv3 构建。"
        "FFmpeg 的版权归其作者所有，以 GPLv3 发布，源码见 [ffmpeg.org](https://ffmpeg.org)。\n\n"
        "> 如果你把本程序和下载好的 `ffmpeg.exe` 一起打包转发给别人，**转发行为本身**需要遵守 GPLv3"
        "（附许可证全文并提供源码获取方式）。建议直接分发本程序，让接收方自行启动下载。",
    ),
])

# ---------------- English ----------------
patch("README.en.md", [
    (
        "**FFmpeg is not required up front** — the app downloads and verifies it on first launch, storing it under `%LOCALAPPDATA%\\CamNexus`.",
        "**FFmpeg is not required up front** — the app downloads it on first launch and then checks that it actually runs (one `-version` call), storing it under `%LOCALAPPDATA%\\CamNexus`.\n\n"
        "> FFmpeg is invoked as a **separate process**: it is not linked, not bundled and not distributed with this app. "
        "It is published by third parties under GPLv3, copyright the FFmpeg authors, source at [ffmpeg.org](https://ffmpeg.org).",
    ),
    (
        "The app itself does not include or distribute FFmpeg; it is downloaded on the user's machine at first launch.",
        "The app itself does not include or distribute FFmpeg; it is downloaded on the user's machine at first launch.\n\n"
        "The app runs FFmpeg from the command line (a separate process, no library linking), using a GPLv3 build. "
        "FFmpeg is copyright its authors and published under GPLv3; source at [ffmpeg.org](https://ffmpeg.org).\n\n"
        "> If you repackage this app together with a downloaded `ffmpeg.exe` and pass it on, **that redistribution** has to comply with GPLv3 "
        "(ship the licence text and an offer of source). Distributing the app alone and letting the recipient download FFmpeg avoids this.",
    ),
])
