//! 因果树与因果链：树作用域、子树渲染、outline 目录、chain 深挖、recall/show 的因果视界行。
//! 语义对齐 respire（memocap CLI）：父=因/子=果，根在前，trivial 出树、父失效提根。

use crate::store::{Record, Store};
use std::collections::{HashMap, HashSet};

/// 纲 L1 概览取父内容头的字数（对齐 respire ANCESTOR_PARENT_CAP=140）
pub const PARENT_L1_CAP: usize = 140;

/// 树作用域内的节点（对齐 respire tree_scope_list：trivial 出树；父不在作用域则提为根）。
/// 结构由 kids（含 "" 键=根集）完整表达。
pub struct TreeScope<'a> {
    pub nodes: Vec<&'a Record>,
    kids: HashMap<String, Vec<String>>,
    pub roots: Vec<String>,
}

impl<'a> TreeScope<'a> {
    pub fn build(store: &'a Store) -> Self {
        let in_scope: HashSet<&str> = store
            .records
            .values()
            .filter(|r| r.importance() != "trivial")
            .map(|r| r.id.as_str())
            .collect();
        let mut nodes: Vec<&Record> = store
            .records
            .values()
            .filter(|r| in_scope.contains(r.id.as_str()))
            .collect();
        nodes.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));

        let mut kids: HashMap<String, Vec<String>> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        for r in &nodes {
            let p = r.parent_id();
            if p.is_empty() || !in_scope.contains(p) {
                roots.push(r.id.clone());
            } else {
                kids.entry(p.to_string()).or_default().push(r.id.clone());
            }
        }
        TreeScope { nodes, kids, roots }
    }

    pub fn children_of(&self, id: &str) -> &[String] {
        self.kids.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn get(&self, id: &str) -> Option<&'a Record> {
        self.nodes.iter().copied().find(|r| r.id == id)
    }
}

/// 带子树深度的节点（tree --json 用）
pub struct TreeNode {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub level: usize,
    pub leaf: bool,
    pub children: Vec<TreeNode>,
}

/// 树渲染（对齐 respire 格式）：
/// 根头 `🌳 因果树根 [id8] title`，节点行 `{prefix}{title} [id8] {kind} 🍃叶?`，
/// 分支 `├─ `/`└─ `，竖线继承用 next_prefix（三层不串行）。
pub fn render_tree(scope: &TreeScope, from: Option<&str>, depth: usize) -> Result<Vec<String>, String> {
    fn label(scope: &TreeScope, id: &str) -> String {
        scope
            .get(id)
            .map(|r| if r.title.is_empty() { id[..id.len().min(8)].to_owned() } else { r.title.clone() })
            .unwrap_or_else(|| id[..id.len().min(8)].to_owned())
    }

    fn render(
        scope: &TreeScope,
        node: &str,
        depth: usize,
        prefix: &str,
        out: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        if !seen.insert(node.to_string()) {
            out.push(format!("{prefix}（环，截断）[{}]", &node[..node.len().min(8)]));
            return;
        }
        let title = label(scope, node);
        let kids: Vec<String> = scope.children_of(node).to_vec();
        let meta = scope.get(node).map(|r| r.kind()).unwrap_or("");
        let leaf = if kids.is_empty() { " 🍃叶" } else { "" };
        out.push(format!("{prefix}{title} [{}] {meta}{leaf}", &node[..node.len().min(8)]));
        if depth == 0 {
            return;
        }
        let n = kids.len();
        for (i, kid) in kids.iter().enumerate() {
            let last = i == n - 1;
            let branch = if last { "└─ " } else { "├─ " };
            // 子层前缀继承 next_prefix：非末子的后代要带竖线，否则三层渲染串行
            let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
            render(scope, kid, depth - 1, &format!("{next_prefix}{branch}"), out, seen);
        }
        seen.remove(node);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    match from {
        Some(f) => {
            if scope.get(f).is_none() {
                return Err(format!("无此节点或已出树: {f}"));
            }
            out.push(format!("🌳 因果树（自 {f}）"));
            render(scope, f, depth, "", &mut out, &mut seen);
        }
        None => {
            if scope.roots.is_empty() {
                return Ok(vec!["（无根节点——库内皆叶，因果树未成形）".into()]);
            }
            for r in &scope.roots {
                out.push(format!("🌳 因果树根 [{}] {}", &r[..r.len().min(8)], label(scope, r)));
                let kids: Vec<String> = scope.children_of(r).to_vec();
                let n = kids.len();
                for (i, kid) in kids.iter().enumerate() {
                    let last = i == n - 1;
                    let branch = if last { "└─ " } else { "├─ " };
                    render(scope, kid, depth.saturating_sub(1), &branch, &mut out, &mut seen);
                }
            }
        }
    }
    Ok(out)
}

/// outline 全深目录：`[id8] title` 逐层缩进（AI 梳理先读此），返回行 + 节点数。
pub fn outline(scope: &TreeScope, from: Option<&str>) -> Result<(Vec<String>, usize), String> {
    fn walk(scope: &TreeScope, node: &str, indent: usize, out: &mut Vec<String>, seen: &mut HashSet<String>) {
        if !seen.insert(node.to_string()) {
            return;
        }
        let title = scope
            .get(node)
            .map(|r| if r.title.is_empty() { node[..node.len().min(8)].to_owned() } else { r.title.clone() })
            .unwrap_or_else(|| node[..node.len().min(8)].to_owned());
        out.push(format!("{}[{}] {}", "  ".repeat(indent), &node[..node.len().min(8)], title));
        for kid in scope.children_of(node) {
            walk(scope, kid, indent + 1, out, seen);
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    match from {
        Some(f) => {
            if scope.get(f).is_none() {
                return Err(format!("无此节点或已出树: {f}"));
            }
            walk(scope, f, 0, &mut out, &mut seen);
        }
        None => {
            for r in &scope.roots {
                walk(scope, r, 0, &mut out, &mut seen);
            }
        }
    }
    let n = out.len();
    Ok((out, n))
}

/// tree --json 结构（全深树，from 定起点）
pub fn tree_json(scope: &TreeScope, from: Option<&str>) -> Result<Vec<TreeNode>, String> {
    fn build(scope: &TreeScope, node: &str, level: usize, seen: &mut HashSet<String>) -> TreeNode {
        let title = scope
            .get(node)
            .map(|r| if r.title.is_empty() { node[..node.len().min(8)].to_owned() } else { r.title.clone() })
            .unwrap_or_else(|| node[..node.len().min(8)].to_owned());
        let kind = scope.get(node).map(|r| r.kind().to_string()).unwrap_or_default();
        let kids: Vec<String> = if seen.insert(node.to_string()) {
            scope.children_of(node).to_vec()
        } else {
            Vec::new()
        };
        let leaf = kids.is_empty();
        TreeNode {
            id: node.to_string(),
            title,
            kind,
            level,
            leaf,
            children: kids.iter().map(|k| build(scope, k, level + 1, seen)).collect(),
        }
    }

    let mut seen = HashSet::new();
    match from {
        Some(f) => {
            if scope.get(f).is_none() {
                return Err(format!("无此节点或已出树: {f}"));
            }
            Ok(vec![build(scope, f, 0, &mut seen)])
        }
        None => Ok(scope.roots.iter().map(|r| build(scope, r, 0, &mut seen)).collect()),
    }
}

/// 因果链深挖（对齐 respire chain）：因链全文（根在前）+ 本条 + 果链 BFS 下探 depth 层。
pub struct ChainView<'a> {
    pub ancestors: Vec<&'a Record>,
    pub entry: &'a Record,
    /// (层级 1..=depth, 记录)，BFS 序
    pub descendants: Vec<(usize, &'a Record)>,
}

pub fn chain_view<'a>(store: &'a Store, id: &str, depth: usize) -> Result<ChainView<'a>, String> {
    let entry = store.records.get(id).ok_or(format!("记忆不存在: {id}"))?;
    let ancestors: Vec<&Record> = store
        .ancestor_chain(id)
        .into_iter()
        .filter_map(|pid| store.records.get(&pid))
        .collect();
    let mut descendants = Vec::new();
    let mut frontier: Vec<(String, usize)> = vec![(id.to_string(), 1)];
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(id.to_string());
    while let Some((cur, lv)) = frontier.pop() {
        if lv > depth {
            continue;
        }
        for c in store.children(&cur) {
            if !seen.insert(c.id.clone()) {
                continue;
            }
            descendants.push((lv, c));
            frontier.push((c.id.clone(), lv + 1));
        }
    }
    Ok(ChainView { ancestors, entry, descendants })
}

/// recall/show 因果视界行（对齐 respire print_causal）：
/// 🔺因链标题链 → ▸纲L1 父内容头 → 🔻果（3 子带摘要 + 各孙标题）→ 超量提示走 chain。
/// 返回行列表（无父无子返回空）。
pub fn causal_lines(store: &Store, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let title_of = |pid: &str| -> String {
        store
            .records
            .get(pid)
            .map(|r| if r.title.is_empty() { pid[..pid.len().min(8)].to_owned() } else { r.title.clone() })
            .unwrap_or_else(|| pid[..pid.len().min(8)].to_owned())
    };

    let chain: Vec<String> = store
        .ancestor_chain(id)
        .into_iter()
        .filter(|pid| store.records.contains_key(pid))
        .collect();
    if !chain.is_empty() {
        let parts: Vec<String> = chain.iter().map(|pid| title_of(pid)).collect();
        out.push(format!("   🔺因链: {} → 本条为其果", parts.join(" ← ")));
        if let Some(pid) = chain.last() {
            if let Some(p) = store.records.get(pid) {
                let head: String = p.content.chars().take(PARENT_L1_CAP).collect::<String>().replace('\n', " ");
                if !head.trim().is_empty() {
                    out.push(format!("   ▸纲L1 「{}」 {head}", title_of(pid)));
                }
            }
        }
    }

    let children = store.children(id);
    if !children.is_empty() {
        out.push(format!("   🔻果: {} 个（本条又为下游之因）", children.len()));
        for c in children.iter().take(3) {
            let title = if c.title.is_empty() { c.id[..c.id.len().min(8)].to_owned() } else { c.title.clone() };
            let summary: String = c.content.chars().take(60).collect::<String>().replace('\n', " ");
            let sep = if summary.is_empty() { String::new() } else { " · ".to_owned() };
            out.push(format!("      └─ [{}] {title}{sep}{summary}", &c.id[..c.id.len().min(8)]));
            let grands = store.children(&c.id);
            let shown = 3usize.min(grands.len());
            for g in grands.iter().take(shown) {
                let gt = if g.title.is_empty() { g.id[..g.id.len().min(8)].to_owned() } else { g.title.clone() };
                out.push(format!("         └─ {gt} [{}]", &g.id[..g.id.len().min(8)]));
            }
            if grands.len() > shown {
                out.push(format!("         …另 {} 个", grands.len() - shown));
            }
        }
        if children.len() > 3 {
            out.push(format!(
                "      …另 {} 个果 → memlite chain {}",
                children.len() - 3,
                &id[..id.len().min(8)]
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("memlite-tree-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 建三层树：纲A → 子B/C → 孙D（D 挂 B 下）
    fn seed() -> (Store, String, String, String, String) {
        let d = tmp();
        let mut s = Store::open(&d);
        let a = s.put_child("纲A正文", "纲A", &[], "", "").unwrap();
        let b = s.put_child("子B正文", "子B", &[], "", &a).unwrap();
        let c = s.put_child("子C正文", "子C", &[], "", &a).unwrap();
        let dd = s.put_child("孙D正文", "孙D", &[], "", &b).unwrap();
        (s, a, b, c, dd)
    }

    #[test]
    fn render_three_levels_prefix_inherits_bar() {
        let (mut s, _a, b, _c, _d) = seed();
        // 子B 下再添一子E：D 为非末子、E 为末子
        let _e = s.put_child("孙E正文", "孙E", &[], "", &b).unwrap();
        let scope = TreeScope::build(&s);
        let lines = render_tree(&scope, None, 3).unwrap();
        let joined = lines.join("\n");
        // respire 累积分支算法（两仓源码一致＋装机 rsrs level=1→3→5 实证）：
        // 子行前缀 = 父前缀(含父分支) + (末子? "   " : "│  ") + 本行分支
        assert!(joined.contains("├─ │  ├─ 孙D"), "非末孙竖线继承错误:\n{joined}");
        assert!(joined.contains("├─    └─ 孙E"), "末子孙空格继承错误:\n{joined}");
        assert!(lines[0].contains("🌳 因果树根 [") && lines[0].contains("纲A"), "根头缺失:\n{joined}");
    }

    #[test]
    fn render_from_subtree_and_depth_zero() {
        let (s, a, b, _c, _d) = seed();
        let scope = TreeScope::build(&s);
        let lines = render_tree(&scope, Some(&a), 0).unwrap();
        // respire from-mode：表头一行 + 自身一行（depth 0 不展子）
        assert_eq!(lines.len(), 2, "depth 0 = 表头+自身: {lines:?}");
        assert!(lines[0].contains("因果树（自") && lines[1].contains("纲A"));
        let lines = render_tree(&scope, Some(&b), 3).unwrap();
        assert!(lines[1].contains("子B") && lines[2].contains("孙D"));
        assert!(render_tree(&scope, Some("nonexist"), 3).is_err());
    }

    #[test]
    fn scope_trivial_out_and_orphan_promoted() {
        let d = tmp();
        let mut s = Store::open(&d);
        let a = s.put_child("纲", "纲", &[], "", "").unwrap();
        let b = s.put_child("子", "子", &[], "", &a).unwrap();
        // 父降为 trivial → 子出作用域链、提为根
        let mut rec = s.get(&a).unwrap().clone();
        rec.extra.insert("importance".into(), serde_json::json!("trivial"));
        let op = Record { ..rec };
        // 直写 trivial 行
        let line = serde_json::to_string(&op).unwrap();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(d.join(crate::store::DATA_FILE))
            .unwrap();
        writeln!(f, "{line}").unwrap();
        drop(f);
        let s2 = Store::open(&d);
        let scope = TreeScope::build(&s2);
        assert!(scope.get(&a).is_none(), "trivial 应出树");
        assert!(scope.roots.contains(&b), "父出树后子应提根");
        assert!(scope.children_of(&a).is_empty(), "出树节点不再持有子");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn outline_full_depth_and_count() {
        let (s, a, _b, _c, _d) = seed();
        let scope = TreeScope::build(&s);
        let (lines, n) = outline(&scope, None).unwrap();
        assert_eq!(n, 4, "全库 4 节点: {lines:?}");
        assert_eq!(lines[0], format!("[{}] 纲A", &a[..8]), "根行=短id+标题（根起 indent 0，对齐 respire collect_outline）");
        let (sub, n2) = outline(&scope, Some(&a)).unwrap();
        assert_eq!(n2, 4, "从纲起全深 4 节点");
        assert_eq!(sub.len(), 4);
    }

    #[test]
    fn chain_view_ancestors_root_first_descendants_bfs() {
        let (s, a, b, _c, d) = seed();
        let v = chain_view(&s, &d, 3).unwrap();
        let anc: Vec<&str> = v.ancestors.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(anc, vec![a.as_str(), b.as_str()], "根在前不含本条");
        assert_eq!(v.entry.id, d);
        assert!(v.descendants.is_empty(), "叶无果");
        let v2 = chain_view(&s, &a, 3).unwrap();
        assert!(v2.ancestors.is_empty());
        assert_eq!(v2.descendants.len(), 3, "B + C + D");
        assert_eq!(v2.descendants[0].0, 1, "直接子层 1");
        assert_eq!(v2.descendants[2].0, 2, "孙层 2");
        assert!(chain_view(&s, "nope", 3).is_err());
    }

    #[test]
    fn causal_lines_show_cause_and_effect() {
        let (s, _a, b, _c, d) = seed();
        let lines = causal_lines(&s, &b);
        let joined = lines.join("\n");
        assert!(joined.contains("🔺因链") && joined.contains("纲A"), "因链行缺失:\n{joined}");
        assert!(joined.contains("▸纲L1"), "纲L1 缺失:\n{joined}");
        assert!(joined.contains("🔻果") && joined.contains("孙D"), "果行缺失:\n{joined}");
        // 根且叶 → 空
        let d2 = tmp();
        let mut s2 = Store::open(&d2);
        let lone = s2.put("孤条", "孤", &[], "").unwrap();
        assert!(causal_lines(&s2, &lone).is_empty());
        std::fs::remove_dir_all(&d2).ok();
    }

    #[test]
    fn causal_lines_lists_overflow_hint() {
        let d = tmp();
        let mut s = Store::open(&d);
        let root = s.put("纲", "R", &[], "").unwrap();
        for i in 0..5 {
            s.put_child(&format!("子{i}"), &format!("S{i}"), &[], "", &root).unwrap();
        }
        let joined = causal_lines(&s, &root).join("\n");
        assert!(joined.contains("…另 2 个果 → memlite chain"), "超量提示缺失:\n{joined}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tree_json_nested() {
        let (s, a, _b, _c, _d) = seed();
        let scope = TreeScope::build(&s);
        let roots = tree_json(&scope, None).unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].children.len(), 2, "纲A 下两子");
        assert!(!roots[0].leaf);
        let sub = tree_json(&scope, Some(&a)).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].id, a);
    }
}
