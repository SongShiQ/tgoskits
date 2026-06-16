//! Controller trait definitions and factory registry.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use ax_kspin::SpinNoIrq;
use ax_lazyinit::LazyInit;
use axfs_ng_vfs::{VfsError, VfsResult};

/// 属性信息（参考 Linux cftype 结构）
#[derive(Clone, Copy)]
pub struct AttrInfo {
    /// 属性名（不含控制器前缀，如 "max" 而非 "pids.max"）
    pub name: &'static str,
    /// 是否只读
    pub read_only: bool,
}

/// 控制器 trait（实例级）
///
/// 每个 CgroupNode 持有控制器实例，用于属性读写
pub trait CgroupController: Send + Sync {
    /// 控制器名称（如 "pids", "cpu"）
    fn name(&self) -> &str;

    /// 是否是 domain 控制器（影响进程承载规则）
    fn is_domain(&self) -> bool {
        false
    }

    /// 读取属性（name 已去掉控制器前缀）
    /// 例如：读取 "pids.max" 时，name 参数为 "max"
    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize>;

    /// 写入属性（name 已去掉控制器前缀）
    fn write_attr(&self, name: &str, data: &[u8]) -> VfsResult<usize>;

    /// 返回该控制器支持的所有属性列表
    fn attr_names(&self) -> &[AttrInfo];

    /// 类型转换支持（用于 downcast）
    fn as_any(&self) -> &dyn core::any::Any;
}

/// 控制器工厂 trait（全局注册）
///
/// 用于创建控制器实例，支持根据 subtree_control 动态创建
pub trait CgroupControllerFactory: Send + Sync {
    /// 控制器名称
    fn name(&self) -> &str;

    /// 是否是 domain 控制器
    fn is_domain(&self) -> bool {
        false
    }

    /// 返回该控制器支持的所有属性列表
    /// 用于不创建实例时查询属性
    fn attr_names(&self) -> &[AttrInfo];

    /// 创建新的控制器实例
    fn new_instance(&self) -> Arc<dyn CgroupController>;
}

/// 全局工厂注册表
static FACTORY_REGISTRY: LazyInit<SpinNoIrq<BTreeMap<String, Arc<dyn CgroupControllerFactory>>>> =
    LazyInit::new();

/// 初始化注册表（在 ax-cgroup::init() 中调用）
pub fn init_registry() {
    FACTORY_REGISTRY.init_once(SpinNoIrq::new(BTreeMap::new()));
}

/// 注册控制器工厂
pub fn register_factory(factory: Arc<dyn CgroupControllerFactory>) {
    let mut registry = FACTORY_REGISTRY
        .get()
        .expect("registry not initialized")
        .lock();
    registry.insert(factory.name().to_string(), factory);
}

/// 获取工厂（用于创建实例）
pub fn get_factory(name: &str) -> Option<Arc<dyn CgroupControllerFactory>> {
    let registry = FACTORY_REGISTRY
        .get()
        .expect("registry not initialized")
        .lock();
    registry.get(name).cloned()
}

/// 获取所有已注册的工厂名称
pub fn all_factory_names() -> Vec<String> {
    let registry = FACTORY_REGISTRY
        .get()
        .expect("registry not initialized")
        .lock();
    registry.keys().cloned().collect()
}

/// 属性名解析工具
///
/// 约束：控制器名不允许包含 '.'
///
/// 支持多种格式：
/// - "pids.max" → ("pids", "max")
/// - "cpu.stat.periods" → ("cpu", "stat.periods")
pub fn parse_attr_name(name: &str) -> Option<(&str, &str)> {
    let dot_pos = name.find('.')?;
    let ctrl_name = &name[..dot_pos];
    let attr_name = &name[dot_pos + 1..];

    // 约束：控制器名不允许包含 '.'
    debug_assert!(
        !ctrl_name.contains('.'),
        "控制器名不应包含 '.': {}",
        ctrl_name
    );

    Some((ctrl_name, attr_name))
}

/// 将字符串写入缓冲区（处理 offset）
pub fn write_to_buf(value: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
    let bytes = value.as_bytes();
    if offset >= bytes.len() {
        return Ok(0);
    }
    let to_copy = core::cmp::min(bytes.len() - offset, buf.len());
    buf[..to_copy].copy_from_slice(&bytes[offset..offset + to_copy]);
    Ok(to_copy)
}
