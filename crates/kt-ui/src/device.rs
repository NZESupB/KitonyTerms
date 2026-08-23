//! 运行时设备类型判定。
//!
//! Android/iOS 的 `target_os` 无法区分手机与平板，而两者需要完全不同的主界面：
//! 手机走触屏专用的 [`crate::components::phone_shell`]，平板与桌面复用
//! [`crate::components::main_shell`]。判定依据是视口短边，旋转、折叠屏展开与
//! 分屏都能正确跟随。

use dioxus::prelude::*;

/// 手机与平板的分界短边（CSS px）。
///
/// 对齐 Android 的 `sw600dp` 平板分界：iPhone 短边最大 430pt，iPad mini 744pt，
/// 600 落在两者之间且留有余量。
pub const PHONE_MAX_MIN_SIDE: f64 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    /// 触屏手机：纵向单列、底部标签栏、软键盘输入。
    Phone,
    /// 桌面与平板：多栏工作台。
    Desktop,
}

impl DeviceClass {
    pub fn is_phone(self) -> bool {
        matches!(self, DeviceClass::Phone)
    }
}

/// 由视口短边判定设备类型。非有限值按平板处理，避免测量异常时把大屏塞进手机布局。
pub fn device_class_for_min_side(min_side: f64) -> DeviceClass {
    if min_side.is_finite() && min_side > 0.0 && min_side < PHONE_MAX_MIN_SIDE {
        DeviceClass::Phone
    } else {
        DeviceClass::Desktop
    }
}

/// 桌面平台恒为 [`DeviceClass::Desktop`]：不起 eval，无运行时开销。
#[cfg(all(
    not(any(target_os = "android", target_os = "ios")),
    not(feature = "phone-preview")
))]
pub fn use_device_class() -> Signal<DeviceClass> {
    use_signal(|| DeviceClass::Desktop)
}

/// 移动平台监听视口短边。初值取 Phone：移动端以手机居多，平板至多在首帧后
/// 立刻切回桌面布局。
///
/// 这个常驻 eval 同时是 `--kt-keyboard-inset`（软键盘遮挡量）的**唯一所有者**：
/// 终端和内嵌编辑器都要按它收缩，而两者不会同时挂载，放在任一方都会漏。
/// Android WebView 默认不改 `innerHeight`，只有 `visualViewport` 会变。
///
/// `phone-preview` 特性让桌面端也走这条路径，用于在开发机上把窗口缩到手机尺寸
/// 预览手机 Shell；正式构建不启用。
#[cfg(any(target_os = "android", target_os = "ios", feature = "phone-preview"))]
pub fn use_device_class() -> Signal<DeviceClass> {
    let mut device_class = use_signal(|| DeviceClass::Phone);

    use_effect(move || {
        spawn(async move {
            let mut eval = dioxus::document::eval(
                r#"
                const viewport = window.visualViewport;

                // 软键盘遮挡量：写给 CSS，终端与内嵌编辑器都据此收缩。
                const applyKeyboardInset = () => {
                    const inset = viewport
                        ? Math.max(0, window.innerHeight - viewport.height - viewport.offsetTop)
                        : 0;
                    document.documentElement.style.setProperty(
                        "--kt-keyboard-inset",
                        Math.round(inset) + "px",
                    );
                };

                const report = () => {
                    applyKeyboardInset();
                    dioxus.send([Math.min(window.innerWidth, window.innerHeight)]);
                };

                window.addEventListener("resize", report);
                window.addEventListener("orientationchange", report);
                // 键盘弹收只动 visualViewport，不会触发 window resize。
                viewport?.addEventListener("resize", applyKeyboardInset);
                viewport?.addEventListener("scroll", applyKeyboardInset);
                report();
                await new Promise(() => {});
                "#,
            );

            while let Ok(payload) = eval.recv::<Vec<f64>>().await {
                let Some(min_side) = payload.first().copied() else {
                    continue;
                };
                let next = device_class_for_min_side(min_side);
                if *device_class.peek() != next {
                    device_class.set(next);
                }
            }
        });
    });

    device_class
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_side_below_threshold_is_a_phone() {
        // iPhone 15 Pro 竖屏/横屏短边均为 393。
        assert_eq!(device_class_for_min_side(393.0), DeviceClass::Phone);
        assert_eq!(device_class_for_min_side(599.0), DeviceClass::Phone);
    }

    #[test]
    fn tablets_and_desktops_keep_the_workbench_layout() {
        // iPad mini 短边 744，iPad Pro 11" 短边 834。
        assert_eq!(device_class_for_min_side(600.0), DeviceClass::Desktop);
        assert_eq!(device_class_for_min_side(744.0), DeviceClass::Desktop);
        assert_eq!(device_class_for_min_side(1440.0), DeviceClass::Desktop);
    }

    #[test]
    fn unmeasurable_viewports_fall_back_to_the_workbench_layout() {
        assert_eq!(device_class_for_min_side(f64::NAN), DeviceClass::Desktop);
        assert_eq!(
            device_class_for_min_side(f64::INFINITY),
            DeviceClass::Desktop
        );
        assert_eq!(device_class_for_min_side(0.0), DeviceClass::Desktop);
        assert_eq!(device_class_for_min_side(-1.0), DeviceClass::Desktop);
    }
}
