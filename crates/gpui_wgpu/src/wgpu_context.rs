// 非WebAssembly平台时导入
#[cfg(not(target_family = "wasm"))]
use anyhow::Context as _;
#[cfg(not(target_family = "wasm"))]
use gpui_util::ResultExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use wgpu::TextureFormat;

/// WGPU图形上下文（封装了所有GPU核心资源）
pub struct WgpuContext {
    /// WGPU实例（整个图形库的入口）
    pub instance: wgpu::Instance,
    /// GPU适配器（对应物理显卡）
    pub adapter: wgpu::Adapter,
    /// GPU逻辑设备（线程安全共享）
    pub device: Arc<wgpu::Device>,
    /// GPU命令队列（线程安全共享）
    pub queue: Arc<wgpu::Queue>,
    /// 是否支持双源混合（用于文字抗锯齿）
    dual_source_blending: bool,
    /// 颜色纹理格式
    color_texture_format: wgpu::TextureFormat,
    /// 设备丢失标记（GPU崩溃/挂起时设为true）
    device_lost: Arc<AtomicBool>,
}

/// 合成器GPU提示信息（用于指定优先使用的显卡）
#[derive(Clone, Copy)]
pub struct CompositorGpuHint {
    /// 显卡厂商ID
    pub vendor_id: u32,
    /// 显卡设备ID
    pub device_id: u32,
}

impl WgpuContext {
    /// 非Web平台：创建WGPU上下文（同步阻塞方式）
    #[cfg(not(target_family = "wasm"))]
    pub fn new(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'_>,
        compositor_gpu: Option<CompositorGpuHint>,
    ) -> anyhow::Result<Self> {
        // 从环境变量ZED_DEVICE_ID读取显卡ID过滤规则
        let device_id_filter = match std::env::var("ZED_DEVICE_ID") {
            Ok(val) => parse_pci_id(&val)
                .context("解析环境变量ZED_DEVICE_ID中的设备ID失败")
                .log_err(),
            Err(std::env::VarError::NotPresent) => None,
            err => {
                err.context("读取环境变量ZED_DEVICE_ID的值失败")
                    .log_err();
                None
            }
        };

        // 通过真实测试表面配置来选择适配器
        // 这是混合GPU系统中判断兼容性的唯一可靠方法
        let (adapter, device, queue, dual_source_blending, color_texture_format) =
            pollster::block_on(Self::select_adapter_and_device(
                &instance,
                device_id_filter,
                surface,
                compositor_gpu.as_ref(),
            ))?;

        // 初始化设备丢失标记
        let device_lost = Arc::new(AtomicBool::new(false));
        device.set_device_lost_callback({
            let device_lost = Arc::clone(&device_lost);
            move |reason, message| {
                log::error!("WGPU设备丢失：原因={reason:?}，信息={message}");
                // 非主动销毁的丢失，标记状态
                if reason != wgpu::DeviceLostReason::Destroyed {
                    device_lost.store(true, Ordering::Relaxed);
                }
            }
        });

        // 打印选中的显卡信息
        log::info!(
            "已选择GPU适配器：{:?}（后端：{:?}）",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        Ok(Self {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            dual_source_blending,
            color_texture_format,
            device_lost,
        })
    }

    /// WebAssembly平台：异步创建WGPU上下文
    #[cfg(target_family = "wasm")]
    pub async fn new_web() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });

        // 请求高性能GPU适配器
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow::anyhow!("请求GPU适配器失败：{e}"))?;

        log::info!(
            "已选择GPU适配器：{:?}（后端：{:?}）",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        let device_lost = Arc::new(AtomicBool::new(false));
        let (device, queue, dual_source_blending, color_texture_format) =
            Self::create_device(&adapter).await?;

        Ok(Self {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            dual_source_blending,
            color_texture_format,
            device_lost,
        })
    }

    /// 创建GPU设备和命令队列
    async fn create_device(
        adapter: &wgpu::Adapter,
    ) -> anyhow::Result<(wgpu::Device, wgpu::Queue, bool, TextureFormat)> {
        // 检查是否支持双源混合功能
        let dual_source_blending = adapter
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING);

        let mut required_features = wgpu::Features::empty();
        if dual_source_blending {
            required_features |= wgpu::Features::DUAL_SOURCE_BLENDING;
        } else {
            log::warn!(
                "当前GPU不支持双源混合，子像素文字抗锯齿功能将被禁用"
            );
        }

        // 选择颜色纹理格式
        let color_atlas_texture_format = Self::select_color_texture_format(adapter)?;

        // 请求创建GPU设备
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("gpui_device"),
                required_features,
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits())
                    .using_alignment(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("创建WGPU设备失败：{e}"))?;

        Ok((
            device,
            queue,
            dual_source_blending,
            color_atlas_texture_format,
        ))
    }

    /// 非Web平台：创建WGPU实例（绑定显示设备）
    #[cfg(not(target_family = "wasm"))]
    pub fn instance(display: Box<dyn wgpu::wgt::WgpuHasDisplayHandle>) -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: Some(display),
        })
    }

    /// 检查当前适配器是否与显示表面兼容
    pub fn check_compatible_with_surface(&self, surface: &wgpu::Surface<'_>) -> anyhow::Result<()> {
        let caps = surface.get_capabilities(&self.adapter);
        if caps.formats.is_empty() {
            let info = self.adapter.get_info();
            anyhow::bail!(
                "适配器 {:?}（后端={:?}，设备ID={:#06x}）与当前窗口的显示表面不兼容",
                info.name,
                info.backend,
                info.device,
            );
        }
        Ok(())
    }

    /// 选择适配器并创建设备，测试表面是否可正常配置
    /// 混合GPU系统中必须通过真实测试判断兼容性（例如NVIDIA虚假报告Wayland支持）
    #[cfg(not(target_family = "wasm"))]
    async fn select_adapter_and_device(
        instance: &wgpu::Instance,
        device_id_filter: Option<u32>,
        surface: &wgpu::Surface<'_>,
        compositor_gpu: Option<&CompositorGpuHint>,
    ) -> anyhow::Result<(
        wgpu::Adapter,
        wgpu::Device,
        wgpu::Queue,
        bool,
        TextureFormat,
    )> {
        // 获取所有可用显卡适配器
        let mut adapters: Vec<_> = instance.enumerate_adapters(wgpu::Backends::all()).await;

        if adapters.is_empty() {
            anyhow::bail!("未找到任何GPU适配器");
        }

        if let Some(device_id) = device_id_filter {
            log::info!("ZED_DEVICE_ID过滤规则：{:#06x}", device_id);
        }

        // 按优先级排序适配器（优先级从高到低）：
        // 1. 环境变量指定的设备ID（用户强制选择）
        // 2. 与合成器匹配的GPU（显示服务器使用的显卡）
        // 3. 设备类型（独立显卡 > 核显 > 其他 > 虚拟显卡 > CPU渲染）
        // 4. 图形后端（Vulkan/Metal/Dx12 > 其他）
        adapters.sort_by_key(|adapter| {
            let info = adapter.get_info();

            // 设备ID为0时无意义，仅非0值用于匹配
            let device_known = info.device != 0;

            // 用户指定优先级
            let user_override: u8 = match device_id_filter {
                Some(id) if device_known && info.device == id => 0,
                _ => 1,
            };

            // 合成器匹配优先级
            let compositor_match: u8 = match compositor_gpu {
                Some(hint)
                    if device_known
                        && info.vendor == hint.vendor_id
                        && info.device == hint.device_id =>
                {
                    0
                }
                _ => 1,
            };

            // 设备类型优先级
            let type_priority: u8 = match info.device_type {
                wgpu::DeviceType::DiscreteGpu => 0,
                wgpu::DeviceType::IntegratedGpu => 1,
                wgpu::DeviceType::Other => 2,
                wgpu::DeviceType::VirtualGpu => 3,
                wgpu::DeviceType::Cpu => 4,
            };

            // 后端优先级
            let backend_priority: u8 = match info.backend {
                wgpu::Backend::Vulkan => 0,
                wgpu::Backend::Metal => 0,
                wgpu::Backend::Dx12 => 0,
                _ => 1,
            };

            (
                user_override,
                compositor_match,
                type_priority,
                backend_priority,
            )
        });

        // 打印所有可用适配器
        log::info!("找到 {} 个GPU适配器：", adapters.len());
        for adapter in &adapters {
            let info = adapter.get_info();
            log::info!(
                "  - {}（厂商ID={:#06x}，设备ID={:#06x}，后端={:?}，类型={:?}）",
                info.name,
                info.vendor,
                info.device,
                info.backend,
                info.device_type,
            );
        }

        // 逐个测试适配器，验证表面配置
        for adapter in adapters {
            let info = adapter.get_info();
            log::info!("测试适配器：{}（{:?}）...", info.name, info.backend);

            match Self::try_adapter_with_surface(&adapter, surface).await {
                Ok((device, queue, dual_source_blending, color_atlas_texture_format)) => {
                    log::info!(
                        "选中GPU（通过配置测试）：{}（{:?}）",
                        info.name,
                        info.backend
                    );
                    return Ok((
                        adapter,
                        device,
                        queue,
                        dual_source_blending,
                        color_atlas_texture_format,
                    ));
                }
                Err(e) => {
                    log::info!(
                        "  适配器 {}（{:?}）测试失败：{}，尝试下一个...",
                        info.name,
                        info.backend,
                        e
                    );
                }
            }
        }

        anyhow::bail!("未找到可配置显示表面的GPU适配器")
    }

    /// 测试适配器与表面的兼容性：创建设备并执行表面配置
    /// 成功则返回可复用的设备和队列
    #[cfg(not(target_family = "wasm"))]
    async fn try_adapter_with_surface(
        adapter: &wgpu::Adapter,
        surface: &wgpu::Surface<'_>,
    ) -> anyhow::Result<(wgpu::Device, wgpu::Queue, bool, TextureFormat)> {
        let caps = surface.get_capabilities(adapter);
        if caps.formats.is_empty() {
            anyhow::bail!("无兼容的表面纹理格式");
        }
        if caps.alpha_modes.is_empty() {
            anyhow::bail!("无兼容的透明模式");
        }

        let (device, queue, dual_source_blending, color_atlas_texture_format) =
            Self::create_device(adapter).await?;
        // 创建错误捕获域，检测配置失败
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        // 用最小尺寸测试表面配置
        let test_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: caps.formats[0],
            width: 64,
            height: 64,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };

        surface.configure(&device, &test_config);

        // 检查是否有配置错误
        let error = error_scope.pop().await;
        if let Some(e) = error {
            anyhow::bail!("表面配置失败：{e}");
        }

        Ok((
            device,
            queue,
            dual_source_blending,
            color_atlas_texture_format,
        ))
    }

    /// 选择合适的颜色纹理格式
    fn select_color_texture_format(adapter: &wgpu::Adapter) -> anyhow::Result<wgpu::TextureFormat> {
        let required_usages = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        // 优先使用Bgra8Unorm格式
        let bgra_features = adapter.get_texture_format_features(wgpu::TextureFormat::Bgra8Unorm);
        if bgra_features.allowed_usages.contains(required_usages) {
            return Ok(wgpu::TextureFormat::Bgra8Unorm);
        }

        // 降级使用Rgba8Unorm格式
        let rgba_features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm);
        if rgba_features.allowed_usages.contains(required_usages) {
            let info = adapter.get_info();
            log::warn!(
                "适配器 {}（{:?}）不支持Bgra8Unorm图集纹理，降级使用Rgba8Unorm格式",
                info.name,
                info.backend,
            );
            return Ok(wgpu::TextureFormat::Rgba8Unorm);
        }

        // 无可用格式，抛出错误
        let info = adapter.get_info();
        Err(anyhow::anyhow!(
            "适配器 {}（{:?}，设备ID={:#06x}）不支持任何可用的颜色图集纹理格式",
            info.name,
            info.backend,
            info.device,
        ))
    }

    /// 获取是否支持双源混合
    pub fn supports_dual_source_blending(&self) -> bool {
        self.dual_source_blending
    }

    /// 获取颜色纹理格式
    pub fn color_texture_format(&self) -> wgpu::TextureFormat {
        self.color_texture_format
    }

    /// 检查GPU设备是否丢失（驱动崩溃、休眠唤醒等导致）
    /// 返回true时需要重新创建上下文
    pub fn device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Relaxed)
    }

    /// 克隆设备丢失标记，供渲染器共享使用
    pub(crate) fn device_lost_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.device_lost)
    }
}

/// 解析PCI设备ID（十六进制4位格式）
#[cfg(not(target_family = "wasm"))]
fn parse_pci_id(id: &str) -> anyhow::Result<u32> {
    let mut id = id.trim();

    // 去除0x前缀
    if id.starts_with("0x") || id.starts_with("0X") {
        id = &id[2..];
    }
    let is_hex_string = id.chars().all(|c| c.is_ascii_hexdigit());
    let is_4_chars = id.len() == 4;
    // 校验格式：必须是4位十六进制数
    anyhow::ensure!(
        is_4_chars && is_hex_string,
        "需要4位十六进制格式的PCI ID"
    );

    u32::from_str_radix(id, 16).context("PCI ID十六进制解析失败")
}

/// 单元测试模块
#[cfg(test)]
mod tests {
    use super::parse_pci_id;

    #[test]
    fn test_parse_device_id() {
        assert!(parse_pci_id("0xABCD").is_ok());
        assert!(parse_pci_id("ABCD").is_ok());
        assert!(parse_pci_id("abcd").is_ok());
        assert!(parse_pci_id("1234").is_ok());
        assert!(parse_pci_id("123").is_err());
        assert_eq!(
            parse_pci_id(&format!("{:x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:X}", 0x1234)).unwrap(),
        );

        assert_eq!(
            parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),
        );
    }
}