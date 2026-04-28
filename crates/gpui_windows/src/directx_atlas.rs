use collections::FxHashMap;
use etagere::BucketedAtlasAllocator;
use parking_lot::Mutex;
use windows::Win32::Graphics::{
    Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    },
    Dxgi::Common::*,
};

use gpui::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTextureList, AtlasTile, Bounds, DevicePixels,
    PlatformAtlas, Point, Size,
};

/// DirectX 纹理图集（Windows 平台渲染用）
/// 内部使用互斥锁保证线程安全
pub(crate) struct DirectXAtlas(Mutex<DirectXAtlasState>);

/// 图集内部状态（所有纹理、分配器、缓存管理）
struct DirectXAtlasState {
    /// Direct3D11 设备
    device: ID3D11Device,
    /// Direct3D11 设备上下文（用于上传纹理数据）
    device_context: ID3D11DeviceContext,
    /// 单色纹理图集（用于单色文字/图标渲染）
    monochrome_textures: AtlasTextureList<DirectXAtlasTexture>,
    /// 彩色纹理图集
    polychrome_textures: AtlasTextureList<DirectXAtlasTexture>,
    /// 子像素抗锯齿纹理图集（用于高清文字）
    subpixel_textures: AtlasTextureList<DirectXAtlasTexture>,
    /// 图集瓦片缓存：Key -> 瓦片信息
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
}

/// 单个 DirectX 纹理图集对象
struct DirectXAtlasTexture {
    /// 纹理唯一标识
    id: AtlasTextureId,
    /// 每像素字节数（1 字节单色 / 4 字节彩色）
    bytes_per_pixel: u32,
    /// 瓦片分配器（管理图集中的区域分配）
    allocator: BucketedAtlasAllocator,
    /// D3D11 2D 纹理对象
    texture: ID3D11Texture2D,
    /// 着色器资源视图（供 GPU 采样使用）
    view: [Option<ID3D11ShaderResourceView>; 1],
    /// 当前活跃的瓦片引用计数（为 0 时可释放纹理）
    live_atlas_keys: u32,
}

impl DirectXAtlas {
    /// 创建 DirectX 纹理图集实例
    pub(crate) fn new(device: &ID3D11Device, device_context: &ID3D11DeviceContext) -> Self {
        DirectXAtlas(Mutex::new(DirectXAtlasState {
            device: device.clone(),
            device_context: device_context.clone(),
            monochrome_textures: Default::default(),
            polychrome_textures: Default::default(),
            subpixel_textures: Default::default(),
            tiles_by_key: Default::default(),
        }))
    }

    /// 根据纹理 ID 获取着色器资源视图（供渲染器使用）
    pub(crate) fn get_texture_view(
        &self,
        id: AtlasTextureId,
    ) -> [Option<ID3D11ShaderResourceView>; 1] {
        let lock = self.0.lock();
        let tex = lock.texture(id);
        tex.view.clone()
    }

    /// 处理 GPU 设备丢失（重置所有纹理和缓存）
    pub(crate) fn handle_device_lost(
        &self,
        device: &ID3D11Device,
        device_context: &ID3D11DeviceContext,
    ) {
        let mut lock = self.0.lock();
        lock.device = device.clone();
        lock.device_context = device_context.clone();
        lock.monochrome_textures = AtlasTextureList::default();
        lock.polychrome_textures = AtlasTextureList::default();
        lock.subpixel_textures = AtlasTextureList::default();
        lock.tiles_by_key.clear();
    }
}

/// 实现 GPUI 平台图集接口
impl PlatformAtlas for DirectXAtlas {
    /// 获取或创建图集瓦片（核心接口）
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<
            Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<AtlasTile>> {
        let mut lock = self.0.lock();
        // 缓存命中：直接返回
        if let Some(tile) = lock.tiles_by_key.get(key) {
            Ok(Some(*tile))
        } else {
            // 缓存未命中：调用生成函数创建纹理数据
            let Some((size, bytes)) = build()? else {
                return Ok(None);
            };
            // 在图集中分配区域
            let tile = lock
                .allocate(size, key.texture_kind())
                .ok_or_else(|| anyhow::anyhow!("failed to allocate"))?;
            let texture = lock.texture(tile.texture_id);
            // 上传像素数据到 GPU
            texture.upload(&lock.device_context, tile.bounds, &bytes);
            lock.tiles_by_key.insert(key.clone(), tile);
            Ok(Some(tile))
        }
    }

    /// 删除瓦片（减少引用计数，空闲时释放纹理）
    fn remove(&self, key: &AtlasKey) {
        let mut lock = self.0.lock();

        let Some(id) = lock.tiles_by_key.remove(key).map(|tile| tile.texture_id) else {
            return;
        };

        // 根据纹理类型找到对应图集
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut lock.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut lock.polychrome_textures,
            AtlasTextureKind::Subpixel => &mut lock.subpixel_textures,
        };

        let Some(texture_slot) = textures.textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            // 减少引用计数
            texture.decrement_ref_count();
            // 无引用时：加入空闲列表
            if texture.is_unreferenced() {
                textures.free_list.push(texture.id.index as usize);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }
}

impl DirectXAtlasState {
    /// 分配纹理瓦片：先尝试复用现有图集，失败则创建新图集
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = match texture_kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
                AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
            };

            // 逆序遍历：优先使用最新创建的图集（空间更充足）
            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
        }

        // 无可用空间：创建新图集
        let texture = self.push_texture(size, texture_kind)?;
        texture.allocate(size)
    }

    /// 创建并添加新的 D3D11 纹理图集
    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> Option<&mut DirectXAtlasTexture> {
        // 默认图集尺寸 1024x1024
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        // Direct3D11 最大纹理尺寸
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        // 最终图集尺寸：在默认和最大值之间
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&DEFAULT_ATLAS_SIZE);
        
        // 根据纹理类型设置格式、字节数
        let pixel_format;
        let bind_flag;
        let bytes_per_pixel;
        match kind {
            AtlasTextureKind::Monochrome => {
                pixel_format = DXGI_FORMAT_R8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 1;
            }
            AtlasTextureKind::Polychrome => {
                pixel_format = DXGI_FORMAT_B8G8R8A8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 4;
            }
            AtlasTextureKind::Subpixel => {
                pixel_format = DXGI_FORMAT_R8G8B8A8_UNORM;
                bind_flag = D3D11_BIND_SHADER_RESOURCE;
                bytes_per_pixel = 4;
            }
        }

        // 创建 D3D11 纹理描述
        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: size.width.0 as u32,
            Height: size.height.0 as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: pixel_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind_flag.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };

        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            // 创建 2D 纹理，设备丢失时返回 None
            self.device
                .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                .ok()?;
        }
        let texture = texture.unwrap();

        // 获取对应类型的图集列表
        let texture_list = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
        };

        // 复用空闲索引
        let index = texture_list.free_list.pop();
        
        // 创建着色器资源视图
        let view = unsafe {
            let mut view = None;
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut view))
                .ok()?;
            [view]
        };

        // 初始化图集纹理
        let atlas_texture = DirectXAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            bytes_per_pixel,
            allocator: BucketedAtlasAllocator::new(device_size_to_etagere(size)),
            texture,
            view,
            live_atlas_keys: 0,
        };

        // 存入图集列表（复用或追加）
        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list.textures.get_mut(ix).unwrap().as_mut()
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list.textures.last_mut().unwrap().as_mut()
        }
    }

    /// 根据 ID 获取纹理（安全解包）
    fn texture(&self, id: AtlasTextureId) -> &DirectXAtlasTexture {
        match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures[id.index as usize]
                .as_ref()
                .unwrap(),
            AtlasTextureKind::Polychrome => &self.polychrome_textures[id.index as usize]
                .as_ref()
                .unwrap(),
            AtlasTextureKind::Subpixel => {
                &self.subpixel_textures[id.index as usize].as_ref().unwrap()
            }
        }
    }
}

impl DirectXAtlasTexture {
    /// 在当前图集中分配一块区域
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(device_size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            bounds: Bounds {
                origin: etagere_point_to_device(allocation.rectangle.min),
                size,
            },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    /// 上传像素数据到 GPU 纹理的指定区域
    fn upload(
        &self,
        device_context: &ID3D11DeviceContext,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) {
        unsafe {
            device_context.UpdateSubresource(
                &self.texture,
                0,
                Some(&D3D11_BOX {
                    left: bounds.left().0 as u32,
                    top: bounds.top().0 as u32,
                    front: 0,
                    right: bounds.right().0 as u32,
                    bottom: bounds.bottom().0 as u32,
                    back: 1,
                }),
                bytes.as_ptr() as _,
                bounds.size.width.to_bytes(self.bytes_per_pixel as u8),
                0,
            );
        }
    }

    /// 减少瓦片引用计数
    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    /// 判断是否无引用（可释放）
    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}

// 坐标/尺寸类型转换工具函数
/// GPUI 设备尺寸 → etagere 分配器尺寸
fn device_size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    etagere::Size::new(size.width.into(), size.height.into())
}

/// etagere 坐标 → GPUI 设备坐标
fn etagere_point_to_device(value: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels::from(value.x),
        y: DevicePixels::from(value.y),
    }
}