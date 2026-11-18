//! 新手引导：步骤表 + 运行状态
//!
//! 设计原则：
//! - 引导**不重写表单**，需要填写时驱动已有窗口，只高亮对应控件；
//! - 除高亮区域外全部锁死，避免用户在引导中把配置点乱；
//! - 演示模式不读写任何真实配置，纯讲解。

/// 高亮锚点名。由各窗口在绘制时登记矩形，引导层再取用。
pub mod anchor {
    pub const TOP_BAR: &str = "top_bar";
    pub const DATA_BAR: &str = "data_bar";
    pub const TILE: &str = "tile";
    pub const SCAN_WINDOW: &str = "scan_window";
    pub const SETTINGS_WINDOW: &str = "settings_window";
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum StepId {
    /// 开场：这个软件是干什么的
    Welcome,
    /// 左上角数据栏
    DataBar,
    /// 顶栏：一排入口
    TopBar,
    /// 画面格子：放大、音量、录制、状态
    VideoArea,
    /// 全屏自由布局
    Fullscreen,
    /// 录像两种模式
    Recording,
    /// 今日事件报告
    Report,
    /// 选择添加方式
    ChooseAdd,
    /// 自动扫描
    Scan,
    /// 手动填写
    Manual,
    /// 收尾
    Done,
}

pub struct Step {
    pub id: StepId,
    pub title: &'static str,
    pub body: &'static str,
    pub anchor: Option<&'static str>,
    /// 主按钮文案；分支步骤（ChooseAdd）用不到
    pub primary: &'static str,
}

pub const STEPS: &[Step] = &[
    Step {
        id: StepId::Welcome,
        title: "先花三分钟，之后随便点",
        body: "CamNexus 把局域网里支持 RTSP 的摄像机收进一个窗口。日常只做三件事：看画面、录像、\
找人找车。这一遍只讲界面怎么用，不会改动你的任何设置。",
        anchor: None,
        primary: "开始",
    },
    Step {
        id: StepId::DataBar,
        title: "左上角：一眼看清状态",
        body: "默认显示在线通道数（绿点表示全部在线）、当前时间、网络收发、CPU 与内存。\
点右边那三个点可以逐项开关——勾选后菜单不会关，可以连着勾几个，点空白处才收起。",
        anchor: Some(anchor::DATA_BAR),
        primary: "下一步",
    },
    Step {
        id: StepId::TopBar,
        title: "顶栏：一排入口",
        body: "从左到右：「回放」打开录像目录；「今日事件」看今天哪些时段有人有车；「识别」开关人体/\
车辆检测（实验功能，需先在设置里下载模型）；「日志」「关于」；最后是「全部重连」「扫描设备」「设置」。\
画面卡住时先点「全部重连」，多数情况能自己恢复。",
        anchor: Some(anchor::TOP_BAR),
        primary: "下一步",
    },
    Step {
        id: StepId::VideoArea,
        title: "画面格子：三处交互",
        body: "双击任意一路放大单看，Esc 退出。右下角两个图标：喇叭控制这一路的声音（同一时刻只播一路）；\
方块是录制开关——连续录制模式下点一下开、再点一下停，录制中变红；事件录制模式下它只是指示灯，\
亮了说明正在落盘。右上角圆点是连接状态。开启识别后，人被黄框框住、车辆是绿框。",
        anchor: Some(anchor::TILE),
        primary: "下一步",
    },
    Step {
        id: StepId::Fullscreen,
        title: "全屏：格子自己排",
        body: "鼠标移到画面右下角会出现全屏按钮。进去之后每路都能拖动、拉伸，靠近边缘会吸附对齐；\
排乱了点「重置布局」。布局会自动记住，下次进来还是这个样子。",
        anchor: Some(anchor::TILE),
        primary: "下一步",
    },
    Step {
        id: StepId::Recording,
        title: "录像：连续录，还是只留证据",
        body: "在「设置 → 录像」里切换两种模式。7×24 连续录制：一直写盘，超出容量上限删最旧的，\
什么都不会漏。事件录制只在有东西动的时候录——人走动、车开过，每段前后各留 5 秒，30 秒内的并成一段；\
站着不动的模特、海报不会触发，平时一个字节都不写。两种模式占用的相机会话数相同。",
        anchor: None,
        primary: "下一步",
    },
    Step {
        id: StepId::Report,
        title: "今日事件：今天录了多少",
        body: "「今日事件」列出每段的通道、起止和时长，底部是合计。显示「进行中」表示此刻正在录；\
显示「中断」说明那段被程序退出打断了——文件还在，只是没写完。",
        anchor: None,
        primary: "下一步",
    },
    Step {
        id: StepId::ChooseAdd,
        title: "添加第一台摄像机",
        body: "只要设备支持 RTSP 就能接进来，不限品牌。自动扫描会找出局域网里的设备并试出可用地址，\
适合不知道地址的情况；手动填写适合你已知地址的场景。",
        anchor: None,
        primary: "",
    },
    Step {
        id: StepId::Scan,
        title: "自动扫描",
        body: "填写网段和账号后点「开始扫描」。密码只在本机内存中使用，不会上传。\
注意多数设备在密码连续错误若干次后会锁定账号，不确定时可先留空只探端口。",
        anchor: Some(anchor::SCAN_WINDOW),
        primary: "下一步",
    },
    Step {
        id: StepId::Manual,
        title: "手动填写",
        body: "在「通道」页填名称、RTSP 地址、账号和密码，地址不确定时可参考右侧常见路径\
（海康 101/102、大华 subtype=0/1）。填完点「保存并重连」。同一台相机的第二路可以填到「子码流」，\
只影响看画面时的带宽，录像仍然走主码流。",
        anchor: Some(anchor::SETTINGS_WINDOW),
        primary: "下一步",
    },
    Step {
        id: StepId::Done,
        title: "可以开始用了",
        body: "引导随时能从「设置」底部的「新手引导」重放。录像按「日期/通道」存放，顶栏「回放」\
直接打开目录。画面卡顿时底部会给出诊断建议。想让软件开机就录，去「设置 → 通用」把「开机自启」\
和「启动后自动录像」都打开。",
        anchor: None,
        primary: "完成",
    },
];

#[derive(Default)]
pub struct Tour {
    /// 当前步骤下标
    pub step: usize,
    /// 演示模式：只看讲解，不打开真实窗口、不写配置
    pub demo: bool,
    /// 已进入过的分支（0=未选，1=扫描，2=手动）
    pub branch: u8,
}

impl Tour {
    pub fn new(demo: bool) -> Self {
        Self {
            step: 0,
            demo,
            branch: 0,
        }
    }

    pub fn current(&self) -> &'static Step {
        &STEPS[self.step.min(STEPS.len() - 1)]
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 >= STEPS.len()
    }

    /// 前进到下一步（跳过 ChooseAdd 的分支选择，由调用方指定）
    pub fn next(&mut self) {
        if self.step + 1 < STEPS.len() {
            self.step += 1;
        }
    }

    pub fn back(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }

    pub fn goto(&mut self, id: StepId) {
        if let Some(i) = STEPS.iter().position(|s| s.id == id) {
            self.step = i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_well_formed() {
        assert_eq!(STEPS.len(), 11, "步骤数变了要同步更新这里的期望值");
        assert_eq!(STEPS[0].id, StepId::Welcome);
        assert_eq!(STEPS[STEPS.len() - 1].id, StepId::Done);
        // 除分支步骤外都要有主按钮文案
        for s in STEPS {
            assert!(!s.title.is_empty(), "步骤缺标题");
            assert!(!s.body.is_empty(), "步骤缺正文");
            if s.id != StepId::ChooseAdd {
                assert!(!s.primary.is_empty(), "{} 缺少主按钮文案", s.title);
            }
        }
        // id 不能重复，否则 goto 会跳错
        let mut ids: Vec<_> = STEPS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), STEPS.len(), "步骤 id 有重复");
    }

    #[test]
    fn every_anchor_is_known() {
        // 锚点名拼错的表现是「高亮区不出现」，很难从界面看出来
        let known = [
            anchor::TOP_BAR,
            anchor::DATA_BAR,
            anchor::TILE,
            anchor::SCAN_WINDOW,
            anchor::SETTINGS_WINDOW,
        ];
        for s in STEPS {
            if let Some(a) = s.anchor {
                assert!(known.contains(&a), "未知锚点 {a}");
            }
        }
    }

    #[test]
    fn navigation_stays_in_range() {
        let mut t = Tour::new(false);
        t.back();
        assert_eq!(t.step, 0);
        for _ in 0..40 {
            t.next();
        }
        assert_eq!(t.step, STEPS.len() - 1);
        assert!(t.is_last());

        t.goto(StepId::Scan);
        assert_eq!(t.current().id, StepId::Scan);
    }
}
