//! 新手引导：步骤表 + 运行状态
//!
//! 设计原则：
//! - 引导**不重写表单**，需要填写时驱动已有窗口，只高亮对应控件；
//! - 除高亮区域外全部锁死，避免用户在引导中把配置点乱；
//! - 演示模式不读写任何真实配置，纯讲解。

/// 高亮锚点名。由各窗口在绘制时登记矩形，引导层再取用。
pub mod anchor {
    pub const TOP_BAR: &str = "top_bar";
    pub const TILE: &str = "tile";
    pub const SCAN_WINDOW: &str = "scan_window";
    pub const SETTINGS_WINDOW: &str = "settings_window";
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepId {
    /// 顶栏：状态、重连、扫描、设置
    TopBar,
    /// 画面区：放大、音量、通道名
    VideoArea,
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
        id: StepId::TopBar,
        title: "顶栏：一切从这里开始",
        body: "左边是在线通道数。右边一排按钮，从左到右：「回放」翻历史录像、「识别」开关人体/车辆检测、\
「日志」「关于」，再往右是「全部重连」「扫描设备」「设置」。发现画面不动时，先点「全部重连」，多数情况能自己恢复。",
        anchor: Some(anchor::TOP_BAR),
        primary: "下一步",
    },
    Step {
        id: StepId::VideoArea,
        title: "画面区：三处交互",
        body: "双击任意一路可放大单看，按 Esc 退出。右下角有两个图标：喇叭控制这一路的声音（同一时刻只播一路），\
摄像机图标是录制开关——点一下开始录、再点一下停止，录制中图标会变红。左下角显示通道名称，\
右上角圆点代表连接状态。打开顶栏「识别」后，画面里的人会被黄框框出、车辆是绿框。",
        anchor: Some(anchor::TILE),
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
        body: "在这里填 RTSP 地址、账号和密码。地址不确定时，可参考右侧常见路径；\
填完点「保存并重连」，画面区就会开始连接。",
        anchor: Some(anchor::SETTINGS_WINDOW),
        primary: "下一步",
    },
    Step {
        id: StepId::Done,
        title: "可以开始用了",
        body: "引导随时可以从「设置 → 新手引导」重新播放。录像按「日期 / 通道」存放，\
顶栏「回放」可以直接打开对应文件夹；运行日志在「日志」里按天查看。\
画面卡顿时，程序会在底部提示可能原因并给出建议按钮。",
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
        assert_eq!(STEPS.len(), 6);
        assert_eq!(STEPS[0].id, StepId::TopBar);
        assert_eq!(STEPS[STEPS.len() - 1].id, StepId::Done);
        // 除分支步骤外都要有主按钮文案
        for s in STEPS {
            if s.id != StepId::ChooseAdd {
                assert!(!s.primary.is_empty(), "{} 缺少主按钮文案", s.title);
            }
        }
    }

    #[test]
    fn navigation_stays_in_range() {
        let mut t = Tour::new(false);
        t.back();
        assert_eq!(t.step, 0);
        for _ in 0..20 {
            t.next();
        }
        assert_eq!(t.step, STEPS.len() - 1);
        assert!(t.is_last());

        t.goto(StepId::Scan);
        assert_eq!(t.current().id, StepId::Scan);
    }
}
