use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use flate2::read::DeflateDecoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::error::{LibraryError, LibraryResult};
use super::limits::{read_stream_limited, ArchiveLimits, ExpansionBudget};

const CONTENT_TYPES_PART: &str = "[Content_Types].xml";
const PRESENTATION_PART: &str = "ppt/presentation.xml";
const RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";
const PRESENTATION_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const SLIDE_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";

#[derive(Debug, Clone)]
pub(super) struct PackageEntry {
    pub name: String,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PptxGraph {
    pub slide_parts: Vec<String>,
    pub chart_parts: Vec<String>,
}

#[derive(Debug, Clone)]
struct ContentTypes {
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct Relationship {
    id: String,
    relationship_type: String,
    target: String,
    external: bool,
}

impl ContentTypes {
    fn declared_type(&self, part_name: &str) -> Option<&str> {
        if let Some(content_type) = self.overrides.get(part_name) {
            return Some(content_type);
        }
        let extension = part_name.rsplit_once('.').map(|(_, extension)| extension)?;
        self.defaults
            .get(&extension.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// 校验 PPTX 包结构。导入校验与建立幻灯片图都按索引预算读取。
pub(super) fn validate_pptx_package(archive: &[u8]) -> LibraryResult<PptxGraph> {
    let entries = read_package_entries(archive, "PPTX", ArchiveLimits::for_index())?;
    validate_pptx_entries(&entries)
}

pub(super) fn pptx_presentation_graph(archive: &[u8]) -> LibraryResult<PptxGraph> {
    validate_pptx_package(archive)
}

/// 生成只读预览用的 DOCX 包；预览预算低于索引预算，超限时明确失败而不是继续展开。
pub(super) fn sanitize_docx_package(archive: &[u8]) -> LibraryResult<Vec<u8>> {
    let entries = read_package_entries(archive, "DOCX", ArchiveLimits::for_preview())?;
    sanitize_entries(entries, OfficeFormat::Docx).map(|entries| write_package_entries(&entries))
}

pub(super) fn sanitize_pptx_package(archive: &[u8]) -> LibraryResult<Vec<u8>> {
    let entries = read_package_entries(archive, "PPTX", ArchiveLimits::for_preview())?;
    let graph = validate_pptx_entries(&entries)?;
    sanitize_entries_with_pptx_graph(entries, &graph).map(|entries| write_package_entries(&entries))
}

fn validate_pptx_entries(entries: &[PackageEntry]) -> LibraryResult<PptxGraph> {
    let content_types = parse_content_types(entries)?;
    if content_types.declared_type(PRESENTATION_PART) != Some(PRESENTATION_CONTENT_TYPE) {
        return Err(LibraryError::ImportFile(
            "PPTX 结构无效：未声明 PowerPoint presentation 主内容类型。".to_string(),
        ));
    }

    let presentation = read_entry(entries, PRESENTATION_PART, "PPTX")?;
    if xml_root_local_name(presentation, "PPTX")? != "presentation" {
        return Err(LibraryError::ImportFile(
            "PPTX 结构无效：ppt/presentation.xml 不是演示文稿定义。".to_string(),
        ));
    }

    let presentation_relationships = read_relationships(entries, PRESENTATION_PART, "PPTX")?
        .ok_or_else(|| {
            LibraryError::ImportFile(
                "PPTX 结构无效：缺少 ppt/_rels/presentation.xml.rels。".to_string(),
            )
        })?;
    let presentation_relationship_map = relationship_map(&presentation_relationships, "PPTX")?;
    validate_relationship_references(
        presentation,
        &presentation_relationship_map,
        PRESENTATION_PART,
        "PPTX",
    )?;

    let slide_ids = presentation_slide_relationship_ids(presentation)?;
    if slide_ids.is_empty() {
        return Err(LibraryError::ImportFile(
            "PPTX 结构无效：p:sldIdLst 没有幻灯片引用。".to_string(),
        ));
    }

    let mut slide_parts = Vec::new();
    let mut chart_parts = Vec::new();
    let mut visited_targets = HashSet::new();
    for relationship_id in slide_ids {
        let relationship = presentation_relationship_map.get(&relationship_id).ok_or_else(|| {
            LibraryError::ImportFile(format!(
                "PPTX 结构无效：p:sldId 引用了不存在的 presentation relationship {relationship_id}。"
            ))
        })?;
        if relationship.external || !relationship_is_slide(&relationship.relationship_type) {
            return Err(LibraryError::ImportFile(
                "PPTX 结构无效：sldIdLst 只能引用包内 slide relationship。".to_string(),
            ));
        }
        let slide_part = resolve_internal_target(PRESENTATION_PART, &relationship.target, "PPTX")?;
        if !visited_targets.insert(slide_part.clone()) {
            return Err(LibraryError::ImportFile(format!(
                "PPTX 结构无效：幻灯片被重复引用：{slide_part}。"
            )));
        }
        require_entry(entries, &slide_part, "PPTX")?;
        if content_types.declared_type(&slide_part) != Some(SLIDE_CONTENT_TYPE) {
            return Err(LibraryError::ImportFile(format!(
                "PPTX 结构无效：幻灯片部件缺少正确内容类型：{slide_part}。"
            )));
        }
        let slide_xml = read_entry(entries, &slide_part, "PPTX")?;
        if xml_root_local_name(slide_xml, "PPTX")? != "sld" {
            return Err(LibraryError::ImportFile(format!(
                "PPTX 结构无效：幻灯片部件根节点不是 sld：{slide_part}。"
            )));
        }
        let slide_relationships =
            read_relationships(entries, &slide_part, "PPTX")?.unwrap_or_default();
        let slide_relationship_map = relationship_map(&slide_relationships, "PPTX")?;
        validate_relationship_references(slide_xml, &slide_relationship_map, &slide_part, "PPTX")?;
        for relationship in slide_relationships {
            if relationship.external {
                continue;
            }
            let target = resolve_internal_target(&slide_part, &relationship.target, "PPTX")?;
            require_entry(entries, &target, "PPTX")?;
            require_declared_content_type(&content_types, &target)?;
            if relationship_is_chart(&relationship.relationship_type) {
                chart_parts.push(target);
            }
        }
        slide_parts.push(slide_part);
    }

    validate_reachable_relationships(
        entries,
        &content_types,
        PRESENTATION_PART,
        &mut HashSet::new(),
    )?;
    chart_parts.sort();
    chart_parts.dedup();
    Ok(PptxGraph {
        slide_parts,
        chart_parts,
    })
}

fn validate_reachable_relationships(
    entries: &[PackageEntry],
    content_types: &ContentTypes,
    part_name: &str,
    visited: &mut HashSet<String>,
) -> LibraryResult<()> {
    if !visited.insert(part_name.to_string()) || read_entry_optional(entries, part_name).is_none() {
        return Ok(());
    }
    let Some(relationships) = read_relationships(entries, part_name, "PPTX")? else {
        return Ok(());
    };
    let relationship_map = relationship_map(&relationships, "PPTX")?;
    if let Some(xml) = read_entry_optional(entries, part_name) {
        validate_relationship_references(xml, &relationship_map, part_name, "PPTX")?;
    }
    for relationship in relationships {
        if relationship.external {
            continue;
        }
        let target = resolve_internal_target(part_name, &relationship.target, "PPTX")?;
        require_entry(entries, &target, "PPTX")?;
        require_declared_content_type(content_types, &target)?;
        validate_reachable_relationships(entries, content_types, &target, visited)?;
    }
    Ok(())
}

fn sanitize_entries_with_pptx_graph(
    entries: Vec<PackageEntry>,
    graph: &PptxGraph,
) -> LibraryResult<Vec<PackageEntry>> {
    let referenced = graph
        .slide_parts
        .iter()
        .chain(graph.chart_parts.iter())
        .cloned()
        .collect::<HashSet<_>>();
    let mut filtered = entries
        .into_iter()
        .filter(|entry| {
            if entry.name.starts_with("ppt/slides/")
                && entry.name.ends_with(".xml")
                && !entry.name.contains("/_rels/")
            {
                return referenced.contains(&entry.name);
            }
            if let Some(slide_part) = relationship_source_part(&entry.name, "ppt/slides") {
                return referenced.contains(&slide_part);
            }
            if entry.name.starts_with("ppt/charts/")
                && entry.name.ends_with(".xml")
                && !entry.name.contains("/_rels/")
            {
                return referenced.contains(&entry.name);
            }
            if let Some(chart_part) = relationship_source_part(&entry.name, "ppt/charts") {
                return referenced.contains(&chart_part);
            }
            !is_removed_part(&entry.name, OfficeFormat::Pptx)
        })
        .collect::<Vec<_>>();
    sanitize_entries_in_place(&mut filtered, OfficeFormat::Pptx)
}

fn relationship_source_part(relationship_part: &str, directory: &str) -> Option<String> {
    let rest = relationship_part.strip_prefix(&format!("{directory}/_rels/"))?;
    let source_name = rest.strip_suffix(".rels")?;
    Some(format!("{directory}/{source_name}"))
}

fn sanitize_entries(
    entries: Vec<PackageEntry>,
    format: OfficeFormat,
) -> LibraryResult<Vec<PackageEntry>> {
    let mut filtered = entries
        .into_iter()
        .filter(|entry| !is_removed_part(&entry.name, format))
        .collect::<Vec<_>>();
    sanitize_entries_in_place(&mut filtered, format)
}

fn sanitize_entries_in_place(
    entries: &mut Vec<PackageEntry>,
    format: OfficeFormat,
) -> LibraryResult<Vec<PackageEntry>> {
    let existing_names = entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<HashSet<_>>();
    let mut removed_relationship_ids = HashSet::new();
    let mut removed_parts = HashSet::new();

    for entry in entries
        .iter_mut()
        .filter(|entry| entry.name.ends_with(".rels"))
    {
        let source_part = source_part_for_relationships(&entry.name)?;
        let relationships = parse_relationships(&entry.contents, format.label())?;
        let mut retained = Vec::new();
        for relationship in relationships {
            let target = if relationship.external {
                None
            } else {
                resolve_internal_target(&source_part, &relationship.target, format.label()).ok()
            };
            let remove = relationship.external
                || relationship_is_blocked(&relationship.relationship_type)
                || target.as_deref().is_some_and(|target| {
                    !existing_names.contains(target)
                        || is_removed_part(target, format)
                        || target == "docProps/thumbnail.jpeg"
                });
            if remove {
                removed_relationship_ids.insert(relationship.id.clone());
                if let Some(target) = target {
                    removed_parts.insert(target);
                }
            } else {
                retained.push(relationship);
            }
        }
        entry.contents = write_relationships(&retained);
    }

    entries.retain(|entry| {
        !removed_parts.contains(&entry.name)
            && !is_removed_part(&entry.name, format)
            && entry.name != "docProps/thumbnail.jpeg"
    });

    let retained_names = entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<HashSet<_>>();
    for entry in entries.iter_mut() {
        if entry.name.ends_with(".xml") && entry.name != CONTENT_TYPES_PART {
            entry.contents =
                strip_relationship_references(&entry.contents, &removed_relationship_ids);
        }
    }
    if let Some(content_types) = entries
        .iter_mut()
        .find(|entry| entry.name == CONTENT_TYPES_PART)
    {
        content_types.contents =
            remove_content_type_overrides(&content_types.contents, &retained_names);
    }
    Ok(std::mem::take(entries))
}

fn is_removed_part(name: &str, format: OfficeFormat) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("/activex/")
        || lower.ends_with("vbaproject.bin")
        || lower.contains("/scripts/")
    {
        return true;
    }
    if lower.contains("/embeddings/") {
        return true;
    }
    let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
    if matches!(
        extension,
        Some("mp3" | "wav" | "m4a" | "mp4" | "mov" | "avi" | "wmv" | "webm")
    ) {
        return true;
    }
    match format {
        OfficeFormat::Docx => lower.contains("/fonts/") && extension == Some("odttf"),
        OfficeFormat::Pptx => {
            lower.contains("/fonts/")
                || (lower.contains("/media/")
                    && matches!(extension, Some("fntdata" | "ttf" | "otf")))
        }
    }
}

fn relationship_is_blocked(relationship_type: &str) -> bool {
    let lower = relationship_type.to_ascii_lowercase();
    [
        "/hyperlink",
        "/oleobject",
        "/package",
        "/video",
        "/audio",
        "/media",
        "/font",
        "/script",
        "/control",
        "/activex",
        "/externallink",
        "/subdocument",
        "/altchunk",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn relationship_is_slide(relationship_type: &str) -> bool {
    relationship_type.to_ascii_lowercase().ends_with("/slide")
}

fn relationship_is_chart(relationship_type: &str) -> bool {
    relationship_type.to_ascii_lowercase().ends_with("/chart")
}

fn require_declared_content_type(
    content_types: &ContentTypes,
    part_name: &str,
) -> LibraryResult<()> {
    if content_types.declared_type(part_name).is_none() {
        return Err(LibraryError::ImportFile(format!(
            "PPTX 结构无效：关系目标缺少内容类型声明：{part_name}。"
        )));
    }
    Ok(())
}

fn parse_content_types(entries: &[PackageEntry]) -> LibraryResult<ContentTypes> {
    let xml = read_entry(entries, CONTENT_TYPES_PART, "PPTX")?;
    if xml_root_local_name(xml, "PPTX")? != "Types" {
        return Err(LibraryError::ImportFile(
            "PPTX 结构无效：[Content_Types].xml 根节点不是 Types。".to_string(),
        ));
    }
    let mut defaults = HashMap::new();
    let mut overrides = HashMap::new();
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let local_name = element.local_name();
                let name = local_name.as_ref();
                if name != "Default" && name != "Override" {
                    buffer.clear();
                    continue;
                }
                let attributes = read_attributes(&element, "PPTX [Content_Types].xml")?;
                if name == "Default" {
                    if let (Some(extension), Some(content_type)) =
                        (attributes.get("Extension"), attributes.get("ContentType"))
                    {
                        defaults.insert(
                            extension.trim_start_matches('.').to_ascii_lowercase(),
                            content_type.to_string(),
                        );
                    }
                } else if let (Some(part_name), Some(content_type)) =
                    (attributes.get("PartName"), attributes.get("ContentType"))
                {
                    overrides.insert(normalize_part_name(part_name)?, content_type.to_string());
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 PPTX [Content_Types].xml：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(ContentTypes {
        defaults,
        overrides,
    })
}

fn presentation_slide_relationship_ids(xml: &[u8]) -> LibraryResult<Vec<String>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut in_slide_list = false;
    let mut ids = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let local_name = element.local_name();
                match local_name.as_ref() {
                    "sldIdLst" => in_slide_list = true,
                    "sldId" if in_slide_list => {
                        let relationships = read_attributes(&element, "PPTX p:sldId")?;
                        let id = relationships
                            .get("r:id")
                            .or_else(|| relationships.get("id"))
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| {
                                LibraryError::ImportFile(
                                    "PPTX 结构无效：p:sldId 缺少 r:id。".to_string(),
                                )
                            })?;
                        ids.push(id.to_string());
                    }
                    _ => {}
                }
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == "sldIdLst" => {
                in_slide_list = false;
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 PPTX presentation 幻灯片列表：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(ids)
}

fn validate_relationship_references(
    xml: &[u8],
    relationships: &HashMap<String, Relationship>,
    part_name: &str,
    format: &str,
) -> LibraryResult<()> {
    for relationship_id in relationship_references(xml, format)? {
        if !relationships.contains_key(&relationship_id) {
            return Err(LibraryError::ImportFile(format!(
                "{format} 结构无效：{part_name} 引用了不存在的关系 {relationship_id}。"
            )));
        }
    }
    Ok(())
}

fn relationship_references(xml: &[u8], format: &str) -> LibraryResult<Vec<String>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut references = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                for result in element.attributes().with_checks(false) {
                    let attribute = result.map_err(|error| {
                        LibraryError::ImportFile(format!("{format} XML 属性无效：{error}"))
                    })?;
                    let key = attribute.key.as_ref();
                    if key.starts_with("r:")
                        && matches!(
                            key.rsplit(':').next(),
                            Some("id" | "embed" | "link" | "pict" | "dm" | "lo")
                        )
                    {
                        let value = normalized_attribute_value(&attribute, format)?;
                        if !value.trim().is_empty() {
                            references.push(value);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} 关系引用：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(references)
}

fn read_relationships(
    entries: &[PackageEntry],
    source_part: &str,
    format: &str,
) -> LibraryResult<Option<Vec<Relationship>>> {
    let relationship_part = relationship_part_name(source_part);
    let Some(contents) = read_entry_optional(entries, &relationship_part) else {
        return Ok(None);
    };
    parse_relationships(contents, format).map(Some)
}

fn parse_relationships(xml: &[u8], format: &str) -> LibraryResult<Vec<Relationship>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut relationships = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() != "Relationship" {
                    buffer.clear();
                    continue;
                }
                let attributes = read_attributes(&element, &format!("{format} relationship"))?;
                let id = attributes.get("Id").cloned().ok_or_else(|| {
                    LibraryError::ImportFile(format!("{format} relationship 缺少 Id。"))
                })?;
                let relationship_type = attributes.get("Type").cloned().ok_or_else(|| {
                    LibraryError::ImportFile(format!("{format} relationship {id} 缺少 Type。"))
                })?;
                let target = attributes.get("Target").cloned().ok_or_else(|| {
                    LibraryError::ImportFile(format!("{format} relationship {id} 缺少 Target。"))
                })?;
                let external = attributes
                    .get("TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("External"))
                    || target_is_external(&target);
                relationships.push(Relationship {
                    id,
                    relationship_type,
                    target,
                    external,
                });
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} relationships：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(relationships)
}

fn relationship_map(
    relationships: &[Relationship],
    format: &str,
) -> LibraryResult<HashMap<String, Relationship>> {
    let mut map = HashMap::new();
    for relationship in relationships {
        if map
            .insert(relationship.id.clone(), relationship.clone())
            .is_some()
        {
            return Err(LibraryError::ImportFile(format!(
                "{format} 结构无效：关系 Id 重复：{}。",
                relationship.id
            )));
        }
    }
    Ok(map)
}

fn read_attributes(
    element: &BytesStart<'_>,
    context: &str,
) -> LibraryResult<HashMap<String, String>> {
    let mut attributes = HashMap::new();
    for result in element.attributes().with_checks(false) {
        let attribute = result
            .map_err(|error| LibraryError::ImportFile(format!("{context} 属性无效：{error}")))?;
        let key = attribute.key.as_ref().to_string();
        let value = normalized_attribute_value(&attribute, context)?;
        attributes.insert(key, value);
    }
    Ok(attributes)
}

fn normalized_attribute_value(
    attribute: &quick_xml::events::attributes::Attribute<'_>,
    context: &str,
) -> LibraryResult<String> {
    attribute
        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map(|value| value.into_owned())
        .map_err(|error| LibraryError::ImportFile(format!("{context} 属性编码无效：{error}")))
}

fn resolve_internal_target(source_part: &str, target: &str, format: &str) -> LibraryResult<String> {
    if target_is_external(target) || target.contains('\\') {
        return Err(LibraryError::ImportFile(format!(
            "{format} 结构无效：关系目标不是包内部部件：{target}。"
        )));
    }
    let target = target.trim().split(['#', '?']).next().unwrap_or_default();
    if target.is_empty() {
        return normalize_part_name(source_part);
    }
    let raw = if target.starts_with('/') {
        target.trim_start_matches('/').to_string()
    } else {
        let directory = source_part
            .rsplit_once('/')
            .map(|(directory, _)| directory)
            .unwrap_or("");
        format!("{directory}/{target}")
    };
    let mut parts = Vec::new();
    for component in raw.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(LibraryError::ImportFile(format!(
                        "{format} 结构无效：关系目标逃逸包根目录。"
                    )));
                }
            }
            component => parts.push(component),
        }
    }
    if parts.is_empty() {
        return Err(LibraryError::ImportFile(format!(
            "{format} 结构无效：关系目标为空。"
        )));
    }
    Ok(parts.join("/"))
}

fn target_is_external(target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    target.starts_with("//")
        || target.starts_with("\\\\")
        || target.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
        })
}

fn relationship_part_name(source_part: &str) -> String {
    let (directory, file_name) = source_part.rsplit_once('/').unwrap_or(("", source_part));
    if directory.is_empty() {
        format!("_rels/{file_name}.rels")
    } else {
        format!("{directory}/_rels/{file_name}.rels")
    }
}

fn source_part_for_relationships(relationship_part: &str) -> LibraryResult<String> {
    if relationship_part == "_rels/.rels" {
        return Ok(String::new());
    }
    let (directory, file_name) = relationship_part.rsplit_once("/_rels/").ok_or_else(|| {
        LibraryError::ImportFile(format!(
            "OOXML 结构无效：relationship 部件路径异常：{relationship_part}。"
        ))
    })?;
    let source_file = file_name.strip_suffix(".rels").ok_or_else(|| {
        LibraryError::ImportFile(format!(
            "OOXML 结构无效：relationship 部件缺少 .rels 后缀：{relationship_part}。"
        ))
    })?;
    Ok(if directory.is_empty() {
        source_file.to_string()
    } else {
        format!("{directory}/{source_file}")
    })
}

fn normalize_part_name(part_name: &str) -> LibraryResult<String> {
    let normalized = part_name.trim().trim_start_matches('/');
    if normalized.is_empty()
        || normalized.contains('\\')
        || normalized.split('/').any(|part| part == "..")
    {
        return Err(LibraryError::ImportFile(format!(
            "OOXML 结构无效：部件名称异常：{part_name}。"
        )));
    }
    Ok(normalized.to_string())
}

fn read_entry<'a>(
    entries: &'a [PackageEntry],
    name: &str,
    format: &str,
) -> LibraryResult<&'a [u8]> {
    read_entry_optional(entries, name)
        .ok_or_else(|| LibraryError::ImportFile(format!("{format} 文件缺少 {name}。")))
}

fn read_entry_optional<'a>(entries: &'a [PackageEntry], name: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.contents.as_slice())
}

fn require_entry(entries: &[PackageEntry], name: &str, format: &str) -> LibraryResult<()> {
    read_entry(entries, name, format).map(|_| ())
}

fn xml_root_local_name(xml: &[u8], format: &str) -> LibraryResult<String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return Ok(element.local_name().as_ref().to_string());
            }
            Ok(Event::Eof) => {
                return Err(LibraryError::ImportFile(format!(
                    "{format} XML 为空或缺少根节点。"
                )))
            }
            Err(error) => {
                return Err(LibraryError::ImportFile(format!(
                    "无法解析 {format} XML：{error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn write_relationships(relationships: &[Relationship]) -> Vec<u8> {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"{RELATIONSHIPS_NAMESPACE}\">"
    );
    for relationship in relationships {
        xml.push_str("<Relationship Id=\"");
        xml.push_str(&escape_xml_attribute(&relationship.id));
        xml.push_str("\" Type=\"");
        xml.push_str(&escape_xml_attribute(&relationship.relationship_type));
        xml.push_str("\" Target=\"");
        xml.push_str(&escape_xml_attribute(&relationship.target));
        xml.push('"');
        if relationship.external {
            xml.push_str(" TargetMode=\"External\"");
        }
        xml.push_str("/>");
    }
    xml.push_str("</Relationships>");
    xml.into_bytes()
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn strip_relationship_references(xml: &[u8], removed_ids: &HashSet<String>) -> Vec<u8> {
    if removed_ids.is_empty() {
        return xml.to_vec();
    }
    let mut output = Vec::with_capacity(xml.len());
    let mut cursor = 0;
    while cursor < xml.len() {
        let Some(relative_start) = xml[cursor..].iter().position(|byte| *byte == b'<') else {
            output.extend_from_slice(&xml[cursor..]);
            break;
        };
        let tag_start = cursor + relative_start;
        output.extend_from_slice(&xml[cursor..tag_start]);
        if xml[tag_start..].starts_with(b"<!--") {
            if let Some(end) = find_bytes(&xml[tag_start + 4..], b"-->") {
                let tag_end = tag_start + 4 + end + 3;
                output.extend_from_slice(&xml[tag_start..tag_end]);
                cursor = tag_end;
                continue;
            }
        }
        if xml[tag_start..].starts_with(b"<![CDATA[") {
            if let Some(end) = find_bytes(&xml[tag_start + 9..], b"]]>") {
                let tag_end = tag_start + 9 + end + 3;
                output.extend_from_slice(&xml[tag_start..tag_end]);
                cursor = tag_end;
                continue;
            }
        }
        let Some(relative_end) = find_tag_end(&xml[tag_start + 1..]) else {
            output.extend_from_slice(&xml[tag_start..]);
            break;
        };
        let tag_end = tag_start + 1 + relative_end + 1;
        let tag = &xml[tag_start..tag_end];
        output.extend_from_slice(&strip_relationship_attributes(tag, removed_ids));
        cursor = tag_end;
    }
    output
}

fn strip_relationship_attributes(tag: &[u8], removed_ids: &HashSet<String>) -> Vec<u8> {
    let tag_text = String::from_utf8_lossy(tag);
    if !tag_text.contains(":id=")
        && !tag_text.contains(":embed=")
        && !tag_text.contains(":link=")
        && !tag_text.contains(":pict=")
        && !tag_text.contains(":dm=")
        && !tag_text.contains(":lo=")
    {
        return tag.to_vec();
    }
    let mut output = String::with_capacity(tag_text.len());
    let mut cursor = 0;
    let bytes = tag_text.as_bytes();
    while cursor < bytes.len() {
        let Some(relative_space) = bytes[cursor..]
            .iter()
            .position(|byte| byte.is_ascii_whitespace())
        else {
            output.push_str(&tag_text[cursor..]);
            break;
        };
        let space = cursor + relative_space;
        output.push_str(&tag_text[cursor..space]);
        if space == bytes.len() - 1 {
            output.push(' ');
            break;
        }
        let mut attribute_start = space;
        while attribute_start < bytes.len() && bytes[attribute_start].is_ascii_whitespace() {
            attribute_start += 1;
        }
        let Some(relative_equal) = bytes[attribute_start..]
            .iter()
            .position(|byte| *byte == b'=')
        else {
            output.push_str(&tag_text[space..]);
            break;
        };
        let equal = attribute_start + relative_equal;
        let name = &tag_text[attribute_start..equal];
        let mut value_start = equal + 1;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        if value_start >= bytes.len() || !matches!(bytes[value_start], b'"' | b'\'') {
            output.push_str(&tag_text[space..=equal]);
            cursor = equal + 1;
            continue;
        }
        let quote = bytes[value_start];
        let Some(relative_quote_end) = bytes[value_start + 1..]
            .iter()
            .position(|byte| *byte == quote)
        else {
            output.push_str(&tag_text[space..]);
            break;
        };
        let value_end = value_start + 1 + relative_quote_end;
        let value = &tag_text[value_start + 1..value_end];
        let relationship_attribute = matches!(
            name.rsplit(':').next(),
            Some("id" | "embed" | "link" | "pict" | "dm" | "lo")
        ) && name.contains(':');
        if !relationship_attribute || !removed_ids.contains(value) {
            output.push_str(&tag_text[space..=value_end]);
        }
        cursor = value_end + 1;
    }
    output.into_bytes()
}

fn remove_content_type_overrides(xml: &[u8], retained_names: &HashSet<String>) -> Vec<u8> {
    let mut output = Vec::with_capacity(xml.len());
    let mut cursor = 0;
    while cursor < xml.len() {
        let Some(relative_start) = xml[cursor..].iter().position(|byte| *byte == b'<') else {
            output.extend_from_slice(&xml[cursor..]);
            break;
        };
        let tag_start = cursor + relative_start;
        output.extend_from_slice(&xml[cursor..tag_start]);
        let Some(relative_end) = find_tag_end(&xml[tag_start + 1..]) else {
            output.extend_from_slice(&xml[tag_start..]);
            break;
        };
        let tag_end = tag_start + 1 + relative_end + 1;
        let tag = &xml[tag_start..tag_end];
        let tag_text = String::from_utf8_lossy(tag);
        let is_override = tag_text.trim_start().starts_with("<Override")
            || tag_text.trim_start().starts_with("<Override ");
        let remove = if is_override {
            extract_attribute(&tag_text, "PartName")
                .and_then(|value| normalize_part_name(&value).ok())
                .is_some_and(|part_name| !retained_names.contains(&part_name))
        } else {
            false
        };
        if !remove {
            output.extend_from_slice(tag);
        }
        cursor = tag_end;
    }
    output
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut search_start = 0;
    while let Some(relative) = tag[search_start..].find(name) {
        let start = search_start + relative;
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let after = start + name.len();
        let after_ok = after < bytes.len() && bytes[after].is_ascii_whitespace();
        if before_ok && after_ok {
            let mut value_start = after;
            while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
            if bytes.get(value_start) != Some(&b'=') {
                search_start = after;
                continue;
            }
            value_start += 1;
            while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
            let quote = *bytes.get(value_start)?;
            if !matches!(quote, b'"' | b'\'') {
                return None;
            }
            let value_start = value_start + 1;
            let value_end = bytes[value_start..]
                .iter()
                .position(|byte| *byte == quote)
                .map(|relative| value_start + relative)?;
            return Some(tag[value_start..value_end].to_string());
        }
        search_start = after;
    }
    None
}

fn find_tag_end(bytes: &[u8]) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in bytes.iter().enumerate() {
        match (*byte, quote) {
            (b'"' | b'\'', None) => quote = Some(*byte),
            (value, Some(expected)) if value == expected => quote = None,
            (b'>', None) => return Some(index),
            _ => {}
        }
    }
    None
}

/// 读取压缩包内所有条目。
///
/// 中央目录里的声明大小完全由文件控制，因此在解压前先按 `limits` 校验输入大小、
/// 单条目声明大小与压缩比，解压时再按累计解压量记账；畸形声明不会带来巨量分配。
fn read_package_entries(
    archive: &[u8],
    format: &str,
    limits: ArchiveLimits,
) -> LibraryResult<Vec<PackageEntry>> {
    let mut budget = ExpansionBudget::new(limits);
    budget.check_input_size(archive.len() as u64)?;
    if !archive.starts_with(b"PK\x03\x04") {
        return Err(LibraryError::ImportFile(format!(
            "文件内容不是有效的 {format}：缺少 OOXML 压缩包结构。"
        )));
    }
    let eocd = find_zip_eocd(archive).ok_or_else(|| {
        LibraryError::ImportFile(format!("{format} 文件结构无效：找不到 ZIP 中央目录。"))
    })?;
    let entry_count = read_u16(archive, eocd + 10)? as usize;
    let mut cursor = read_u32(archive, eocd + 16)? as usize;
    // 每个中央目录项至少 46 字节，按文件长度收敛预分配，避免畸形声明带来的空分配。
    let mut entries = Vec::with_capacity(entry_count.min(archive.len() / 46 + 1));
    let mut names = HashSet::new();

    for _ in 0..entry_count {
        if read_u32(archive, cursor)? != 0x0201_4b50 {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：中央目录项损坏。"
            )));
        }
        let flags = read_u16(archive, cursor + 8)?;
        if flags & 0x0001 != 0 {
            return Err(LibraryError::ImportFile(format!(
                "{format} 已加密，无法导入。"
            )));
        }
        let compression = read_u16(archive, cursor + 10)?;
        let expected_crc = read_u32(archive, cursor + 16)?;
        let compressed_size = read_u32(archive, cursor + 20)? as usize;
        let uncompressed_size = read_u32(archive, cursor + 24)? as usize;
        let name_length = read_u16(archive, cursor + 28)? as usize;
        let extra_length = read_u16(archive, cursor + 30)? as usize;
        let comment_length = read_u16(archive, cursor + 32)? as usize;
        let local_header_offset = read_u32(archive, cursor + 42)? as usize;
        if compressed_size == u32::MAX as usize || uncompressed_size == u32::MAX as usize {
            return Err(LibraryError::ImportFile(format!(
                "暂不支持 ZIP64 格式的 {format} 文件。"
            )));
        }
        let name_start = cursor + 46;
        let name_end = name_start.checked_add(name_length).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：文件名长度溢出。"))
        })?;
        let name_bytes = archive.get(name_start..name_end).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：文件名超出范围。"))
        })?;
        let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：文件名不是有效 UTF-8。"))
        })?;
        let normalized_name = normalize_part_name(&name)?;
        if !names.insert(normalized_name.clone()) {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：部件名称重复：{normalized_name}。"
            )));
        }
        // 解压之前先按声明值筛选，压缩炸弹与畸形的 0 压缩声明都在这里被拒绝。
        budget.check_entry_size(&normalized_name, uncompressed_size as u64)?;
        budget.check_compression_ratio(
            &normalized_name,
            compressed_size as u64,
            uncompressed_size as u64,
        )?;
        let data_start = zip_local_data_start(archive, local_header_offset, format)?;
        let data_end = data_start.checked_add(compressed_size).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：正文长度溢出。"))
        })?;
        let compressed = archive.get(data_start..data_end).ok_or_else(|| {
            LibraryError::ImportFile(format!("{format} 文件结构无效：正文超出文件范围。"))
        })?;
        let contents = match compression {
            0 => {
                let contents = compressed.to_vec();
                budget.charge(&normalized_name, contents.len())?;
                contents
            }
            8 => read_stream_limited(
                DeflateDecoder::new(Cursor::new(compressed)),
                &normalized_name,
                uncompressed_size as u64,
                &mut budget,
            )?,
            method => {
                return Err(LibraryError::ImportFile(format!(
                    "{format} 使用了不支持的压缩方式：{method}。"
                )))
            }
        };
        if contents.len() != uncompressed_size {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：部件 {normalized_name} 的解压长度不一致。"
            )));
        }
        if expected_crc != 0 && crc32(&contents) != expected_crc {
            return Err(LibraryError::ImportFile(format!(
                "{format} 文件结构无效：部件 {normalized_name} 的 CRC 不匹配。"
            )));
        }
        entries.push(PackageEntry {
            name: normalized_name,
            contents,
        });
        cursor = name_end
            .checked_add(extra_length)
            .and_then(|value| value.checked_add(comment_length))
            .ok_or_else(|| {
                LibraryError::ImportFile(format!("{format} 文件结构无效：目录项长度溢出。"))
            })?;
    }
    Ok(entries)
}

fn write_package_entries(entries: &[PackageEntry]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central_entries = Vec::new();
    for entry in entries {
        let local_offset = archive.len() as u32;
        let name = entry.name.as_bytes();
        let crc = crc32(&entry.contents);
        push_u32(&mut archive, 0x0403_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 0x0800);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, crc);
        push_u32(&mut archive, entry.contents.len() as u32);
        push_u32(&mut archive, entry.contents.len() as u32);
        push_u16(&mut archive, name.len() as u16);
        push_u16(&mut archive, 0);
        archive.extend_from_slice(name);
        archive.extend_from_slice(&entry.contents);

        let mut central = Vec::new();
        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0x0800);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, crc);
        push_u32(&mut central, entry.contents.len() as u32);
        push_u32(&mut central, entry.contents.len() as u32);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, local_offset);
        central.extend_from_slice(name);
        central_entries.push(central);
    }
    let central_offset = archive.len() as u32;
    for entry in &central_entries {
        archive.extend_from_slice(entry);
    }
    let central_size = archive.len() as u32 - central_offset;
    push_u32(&mut archive, 0x0605_4b50);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, entries.len() as u16);
    push_u16(&mut archive, entries.len() as u16);
    push_u32(&mut archive, central_size);
    push_u32(&mut archive, central_offset);
    push_u16(&mut archive, 0);
    archive
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn find_zip_eocd(archive: &[u8]) -> Option<usize> {
    let search_start = archive.len().saturating_sub(65_557);
    archive[search_start..]
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|index| search_start + index)
}

fn zip_local_data_start(
    archive: &[u8],
    local_header_offset: usize,
    format: &str,
) -> LibraryResult<usize> {
    if read_u32(archive, local_header_offset)? != 0x0403_4b50 {
        return Err(LibraryError::ImportFile(format!(
            "{format} 文件结构无效：本地文件头损坏。"
        )));
    }
    let name_length = read_u16(archive, local_header_offset + 26)? as usize;
    let extra_length = read_u16(archive, local_header_offset + 28)? as usize;
    local_header_offset
        .checked_add(30)
        .and_then(|value| value.checked_add(name_length))
        .and_then(|value| value.checked_add(extra_length))
        .ok_or_else(|| LibraryError::ImportFile(format!("{format} 文件结构无效：本地头长度溢出。")))
}

fn read_u16(bytes: &[u8], offset: usize) -> LibraryResult<u16> {
    let value = bytes.get(offset..offset + 2).ok_or_else(|| {
        LibraryError::ImportFile("OOXML 文件结构无效：文件意外结束。".to_string())
    })?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> LibraryResult<u32> {
    let value = bytes.get(offset..offset + 4).ok_or_else(|| {
        LibraryError::ImportFile("OOXML 文件结构无效：文件意外结束。".to_string())
    })?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Debug, Clone, Copy)]
enum OfficeFormat {
    Docx,
    Pptx,
}

impl OfficeFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Docx => "DOCX",
            Self::Pptx => "PPTX",
        }
    }
}
