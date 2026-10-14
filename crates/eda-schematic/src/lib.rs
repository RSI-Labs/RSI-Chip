//! API-first editable schematic database.
//!
//! The types here are intentionally independent of a GUI toolkit. A frontend
//! can use hit testing, transforms, net extraction, and SVG rendering while
//! automation uses the same deterministic editing API.

use std::collections::{BTreeMap, BTreeSet};

pub mod browser;
pub use browser::{SchematicBrowserServer, SchematicBrowserService};

const GEOMETRY_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    fn distance_to_segment(self, start: Self, end: Self) -> f64 {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let length_squared = dx * dx + dy * dy;
        if length_squared <= GEOMETRY_EPSILON {
            return self.distance(start);
        }
        let t =
            (((self.x - start.x) * dx + (self.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
        self.distance(Point::new(start.x + t * dx, start.y + t * dy))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn degrees(self) -> u16 {
        match self {
            Self::R0 => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }

    fn transform(self, width: f64, height: f64, mirror_x: bool, point: Point) -> Point {
        let mut x = if mirror_x { width - point.x } else { point.x };
        let mut y = point.y;
        match self {
            Self::R0 => {}
            Self::R90 => {
                std::mem::swap(&mut x, &mut y);
                x = height - x;
            }
            Self::R180 => {
                x = width - x;
                y = height - y;
            }
            Self::R270 => {
                std::mem::swap(&mut x, &mut y);
                y = width - y;
            }
        }
        Point::new(x, y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
    InOut,
}

impl PortDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::InOut => "inout",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolPin {
    pub name: String,
    pub position: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub pins: Vec<SymbolPin>,
}

impl Symbol {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("symbol name cannot be empty".into());
        }
        if !self.width.is_finite() || self.width <= 0.0 {
            return Err(format!("symbol '{}' width must be positive", self.name));
        }
        if !self.height.is_finite() || self.height <= 0.0 {
            return Err(format!("symbol '{}' height must be positive", self.name));
        }
        let mut names = BTreeSet::new();
        for pin in &self.pins {
            if pin.name.trim().is_empty() {
                return Err(format!("symbol '{}' has an empty pin name", self.name));
            }
            if !names.insert(pin.name.clone()) {
                return Err(format!(
                    "symbol '{}' has duplicate pin '{}'",
                    self.name, pin.name
                ));
            }
            if !pin.position.x.is_finite()
                || !pin.position.y.is_finite()
                || pin.position.x < 0.0
                || pin.position.y < 0.0
                || pin.position.x > self.width
                || pin.position.y > self.height
            {
                return Err(format!(
                    "symbol '{}' pin '{}' is outside its outline",
                    self.name, pin.name
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceTransform {
    pub origin: Point,
    pub rotation: Rotation,
    pub mirror_x: bool,
}

impl Default for InstanceTransform {
    fn default() -> Self {
        Self {
            origin: Point::new(0.0, 0.0),
            rotation: Rotation::R0,
            mirror_x: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchematicInstance {
    pub name: String,
    pub symbol: String,
    pub transform: InstanceTransform,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub id: String,
    pub start: Point,
    pub end: Point,
}

impl Wire {
    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("wire id cannot be empty".into());
        }
        if !self.start.x.is_finite()
            || !self.start.y.is_finite()
            || !self.end.x.is_finite()
            || !self.end.y.is_finite()
        {
            return Err(format!("wire '{}' has non-finite geometry", self.id));
        }
        if self.start.distance(self.end) <= GEOMETRY_EPSILON {
            return Err(format!("wire '{}' has zero length", self.id));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchematicPort {
    pub name: String,
    pub direction: PortDirection,
    pub position: Point,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchematicDocument {
    pub title: String,
    pub symbols: BTreeMap<String, Symbol>,
    pub instances: BTreeMap<String, SchematicInstance>,
    pub wires: BTreeMap<String, Wire>,
    pub ports: BTreeMap<String, SchematicPort>,
}

impl SchematicDocument {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn add_symbol(&mut self, symbol: Symbol) -> Result<(), String> {
        symbol.validate()?;
        if self.symbols.contains_key(&symbol.name) {
            return Err(format!("duplicate symbol '{}'", symbol.name));
        }
        self.symbols.insert(symbol.name.clone(), symbol);
        Ok(())
    }

    pub fn add_instance(
        &mut self,
        name: impl Into<String>,
        symbol: &str,
        transform: InstanceTransform,
    ) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("instance name cannot be empty".into());
        }
        if !self.symbols.contains_key(symbol) {
            return Err(format!("unknown symbol '{symbol}'"));
        }
        if !transform.origin.x.is_finite() || !transform.origin.y.is_finite() {
            return Err(format!("instance '{}' has non-finite origin", name));
        }
        if self.instances.contains_key(&name) {
            return Err(format!("duplicate instance '{name}'"));
        }
        self.instances.insert(
            name.clone(),
            SchematicInstance {
                name,
                symbol: symbol.to_string(),
                transform,
            },
        );
        Ok(())
    }

    pub fn move_instance(&mut self, name: &str, origin: Point) -> Result<(), String> {
        if !origin.x.is_finite() || !origin.y.is_finite() {
            return Err("instance origin must be finite".into());
        }
        let instance = self
            .instances
            .get_mut(name)
            .ok_or_else(|| format!("unknown instance '{name}'"))?;
        instance.transform.origin = origin;
        Ok(())
    }

    pub fn set_instance_transform(
        &mut self,
        name: &str,
        rotation: Rotation,
        mirror_x: bool,
    ) -> Result<(), String> {
        let instance = self
            .instances
            .get_mut(name)
            .ok_or_else(|| format!("unknown instance '{name}'"))?;
        instance.transform.rotation = rotation;
        instance.transform.mirror_x = mirror_x;
        Ok(())
    }

    pub fn remove_instance(&mut self, name: &str) -> Result<SchematicInstance, String> {
        self.instances
            .remove(name)
            .ok_or_else(|| format!("unknown instance '{name}'"))
    }

    pub fn add_wire(&mut self, start: Point, end: Point) -> Result<String, String> {
        let mut sequence = self.wires.len() + 1;
        loop {
            let id = format!("W{sequence}");
            if !self.wires.contains_key(&id) {
                let wire = Wire {
                    id: id.clone(),
                    start,
                    end,
                };
                wire.validate()?;
                self.wires.insert(id.clone(), wire);
                return Ok(id);
            }
            sequence += 1;
        }
    }

    pub fn remove_wire(&mut self, id: &str) -> Result<Wire, String> {
        self.wires
            .remove(id)
            .ok_or_else(|| format!("unknown wire '{id}'"))
    }

    pub fn add_port(
        &mut self,
        name: impl Into<String>,
        direction: PortDirection,
        position: Point,
    ) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("port name cannot be empty".into());
        }
        if !position.x.is_finite() || !position.y.is_finite() {
            return Err(format!("port '{}' position must be finite", name));
        }
        if self.ports.contains_key(&name) {
            return Err(format!("duplicate port '{name}'"));
        }
        self.ports.insert(
            name.clone(),
            SchematicPort {
                name,
                direction,
                position,
            },
        );
        Ok(())
    }

    pub fn pin_position(&self, instance: &str, pin: &str) -> Result<Point, String> {
        let instance = self
            .instances
            .get(instance)
            .ok_or_else(|| format!("unknown instance '{instance}'"))?;
        let symbol = self
            .symbols
            .get(&instance.symbol)
            .ok_or_else(|| format!("unknown symbol '{}'", instance.symbol))?;
        let symbol_pin = symbol
            .pins
            .iter()
            .find(|candidate| candidate.name == pin)
            .ok_or_else(|| format!("symbol '{}' has no pin '{pin}'", symbol.name))?;
        let local = instance.transform.rotation.transform(
            symbol.width,
            symbol.height,
            instance.transform.mirror_x,
            symbol_pin.position,
        );
        Ok(Point::new(
            instance.transform.origin.x + local.x,
            instance.transform.origin.y + local.y,
        ))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("schematic title cannot be empty".into());
        }
        for symbol in self.symbols.values() {
            symbol.validate()?;
        }
        for instance in self.instances.values() {
            if !self.symbols.contains_key(&instance.symbol) {
                return Err(format!(
                    "instance '{}' references unknown symbol '{}'",
                    instance.name, instance.symbol
                ));
            }
        }
        for wire in self.wires.values() {
            wire.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchematicConnection {
    Pin { instance: String, pin: String },
    Port { port: String },
    WireEnd { wire: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchematicNet {
    pub name: String,
    pub port_aliases: Vec<String>,
    pub connections: Vec<SchematicConnection>,
}

#[derive(Clone, Debug)]
struct NetNode {
    point: Point,
    connection: SchematicConnection,
}

impl SchematicDocument {
    pub fn extract_nets(&self) -> Result<Vec<SchematicNet>, String> {
        self.validate()?;
        let mut nodes = Vec::new();
        for instance in self.instances.values() {
            let symbol = self
                .symbols
                .get(&instance.symbol)
                .ok_or_else(|| format!("unknown symbol '{}'", instance.symbol))?;
            for pin in &symbol.pins {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    pin.position,
                );
                nodes.push(NetNode {
                    point: Point::new(
                        instance.transform.origin.x + local.x,
                        instance.transform.origin.y + local.y,
                    ),
                    connection: SchematicConnection::Pin {
                        instance: instance.name.clone(),
                        pin: pin.name.clone(),
                    },
                });
            }
        }
        for port in self.ports.values() {
            nodes.push(NetNode {
                point: port.position,
                connection: SchematicConnection::Port {
                    port: port.name.clone(),
                },
            });
        }
        for wire in self.wires.values() {
            for point in [wire.start, wire.end] {
                nodes.push(NetNode {
                    point,
                    connection: SchematicConnection::WireEnd {
                        wire: wire.id.clone(),
                    },
                });
            }
        }
        let mut parent: Vec<usize> = (0..nodes.len()).collect();
        fn find(parent: &mut [usize], mut index: usize) -> usize {
            while parent[index] != index {
                parent[index] = parent[parent[index]];
                index = parent[index];
            }
            index
        }
        fn union(parent: &mut [usize], left: usize, right: usize) {
            let left = find(parent, left);
            let right = find(parent, right);
            if left != right {
                parent[right] = left;
            }
        }
        for wire in self.wires.values() {
            let attached: Vec<_> = nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| {
                    node.point.distance_to_segment(wire.start, wire.end) <= GEOMETRY_EPSILON
                })
                .map(|(index, _)| index)
                .collect();
            if let Some(first) = attached.first() {
                for index in attached.iter().skip(1) {
                    union(&mut parent, *first, *index);
                }
            }
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (index, _) in nodes.iter().enumerate() {
            groups
                .entry(find(&mut parent, index))
                .or_default()
                .push(index);
        }
        let mut nets = Vec::new();
        let mut anonymous = 1_usize;
        for members in groups.into_values() {
            let port_aliases: Vec<_> = members
                .iter()
                .filter_map(|index| match &nodes[*index].connection {
                    SchematicConnection::Port { port } => Some(port.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let name = port_aliases
                .first()
                .cloned()
                .unwrap_or_else(|| format!("net{anonymous}"));
            if port_aliases.is_empty() {
                anonymous += 1;
            }
            let mut connections = members
                .iter()
                .map(|index| nodes[*index].connection.clone())
                .collect::<Vec<_>>();
            connections.sort_by_key(|connection| format!("{connection:?}"));
            nets.push(SchematicNet {
                name,
                port_aliases,
                connections,
            });
        }
        nets.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(nets)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchematicHit {
    Pin {
        instance: String,
        pin: String,
        position: Point,
    },
    Port {
        name: String,
        position: Point,
    },
    Instance {
        name: String,
    },
    Wire {
        id: String,
        position: Point,
    },
}

impl SchematicDocument {
    pub fn hit_test(&self, point: Point, radius: f64) -> Result<Option<SchematicHit>, String> {
        if !point.x.is_finite() || !point.y.is_finite() || !radius.is_finite() || radius < 0.0 {
            return Err("hit-test point and radius must be finite/non-negative".into());
        }
        let mut best: Option<(f64, SchematicHit)> = None;
        let consider =
            |distance: f64, hit: SchematicHit, best: &mut Option<(f64, SchematicHit)>| {
                let closer = match best {
                    Some((old, _)) => distance < *old,
                    None => true,
                };
                if distance <= radius && closer {
                    *best = Some((distance, hit));
                }
            };
        for instance in self.instances.values() {
            let Some(symbol) = self.symbols.get(&instance.symbol) else {
                continue;
            };
            for pin in &symbol.pins {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    pin.position,
                );
                let absolute = Point::new(
                    instance.transform.origin.x + local.x,
                    instance.transform.origin.y + local.y,
                );
                consider(
                    point.distance(absolute),
                    SchematicHit::Pin {
                        instance: instance.name.clone(),
                        pin: pin.name.clone(),
                        position: absolute,
                    },
                    &mut best,
                );
            }
        }
        for port in self.ports.values() {
            consider(
                point.distance(port.position),
                SchematicHit::Port {
                    name: port.name.clone(),
                    position: port.position,
                },
                &mut best,
            );
        }
        for wire in self.wires.values() {
            consider(
                point.distance_to_segment(wire.start, wire.end),
                SchematicHit::Wire {
                    id: wire.id.clone(),
                    position: point,
                },
                &mut best,
            );
        }
        for instance in self.instances.values() {
            let Some(symbol) = self.symbols.get(&instance.symbol) else {
                continue;
            };
            let corners = [
                Point::new(0.0, 0.0),
                Point::new(symbol.width, 0.0),
                Point::new(symbol.width, symbol.height),
                Point::new(0.0, symbol.height),
            ]
            .map(|corner| {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    corner,
                );
                Point::new(
                    instance.transform.origin.x + local.x,
                    instance.transform.origin.y + local.y,
                )
            });
            let min_x = corners
                .iter()
                .map(|corner| corner.x)
                .fold(f64::INFINITY, f64::min);
            let max_x = corners
                .iter()
                .map(|corner| corner.x)
                .fold(f64::NEG_INFINITY, f64::max);
            let min_y = corners
                .iter()
                .map(|corner| corner.y)
                .fold(f64::INFINITY, f64::min);
            let max_y = corners
                .iter()
                .map(|corner| corner.y)
                .fold(f64::NEG_INFINITY, f64::max);
            if point.x >= min_x
                && point.x <= max_x
                && point.y >= min_y
                && point.y <= max_y
                && best.is_none()
            {
                best = Some((
                    0.0,
                    SchematicHit::Instance {
                        name: instance.name.clone(),
                    },
                ));
            }
        }
        Ok(best.map(|(_, hit)| hit))
    }

    pub fn write_svg(&self) -> Result<String, String> {
        self.validate()?;
        let mut points = Vec::new();
        for instance in self.instances.values() {
            let Some(symbol) = self.symbols.get(&instance.symbol) else {
                continue;
            };
            for pin in &symbol.pins {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    pin.position,
                );
                points.push(Point::new(
                    instance.transform.origin.x + local.x,
                    instance.transform.origin.y + local.y,
                ));
            }
            for corner in [
                Point::new(0.0, 0.0),
                Point::new(symbol.width, 0.0),
                Point::new(symbol.width, symbol.height),
                Point::new(0.0, symbol.height),
            ] {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    corner,
                );
                points.push(Point::new(
                    instance.transform.origin.x + local.x,
                    instance.transform.origin.y + local.y,
                ));
            }
        }
        points.extend(self.wires.values().flat_map(|wire| [wire.start, wire.end]));
        points.extend(self.ports.values().map(|port| port.position));
        let min_x = points.iter().map(|point| point.x).fold(0.0_f64, f64::min) - 20.0;
        let max_x = points.iter().map(|point| point.x).fold(0.0_f64, f64::max) + 20.0;
        let min_y = points.iter().map(|point| point.y).fold(0.0_f64, f64::min) - 20.0;
        let max_y = points.iter().map(|point| point.y).fold(0.0_f64, f64::max) + 20.0;
        let width = (max_x - min_x).max(10.0);
        let height = (max_y - min_y).max(10.0);
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.3}\" height=\"{height:.3}\" viewBox=\"{min_x:.3} {min_y:.3} {width:.3} {height:.3}\">\n"
        );
        out.push_str("<rect fill=\"white\" stroke=\"none\" x=\"{min_x:.3}\" y=\"{min_y:.3}\" width=\"{width:.3}\" height=\"{height:.3}\"/>\n");
        for wire in self.wires.values() {
            out.push_str(&format!(
                "<line x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" stroke=\"#111827\" stroke-width=\"0.8\"/>\n",
                wire.start.x, wire.start.y, wire.end.x, wire.end.y
            ));
        }
        for instance in self.instances.values() {
            let Some(symbol) = self.symbols.get(&instance.symbol) else {
                continue;
            };
            let corners = [
                Point::new(0.0, 0.0),
                Point::new(symbol.width, 0.0),
                Point::new(symbol.width, symbol.height),
                Point::new(0.0, symbol.height),
            ]
            .map(|corner| {
                let local = instance.transform.rotation.transform(
                    symbol.width,
                    symbol.height,
                    instance.transform.mirror_x,
                    corner,
                );
                Point::new(
                    instance.transform.origin.x + local.x,
                    instance.transform.origin.y + local.y,
                )
            });
            let x = corners
                .iter()
                .map(|point| point.x)
                .fold(f64::INFINITY, f64::min);
            let y = corners
                .iter()
                .map(|point| point.y)
                .fold(f64::INFINITY, f64::min);
            let width = corners
                .iter()
                .map(|point| point.x)
                .fold(f64::NEG_INFINITY, f64::max)
                - x;
            let height = corners
                .iter()
                .map(|point| point.y)
                .fold(f64::NEG_INFINITY, f64::max)
                - y;
            out.push_str(&format!(
                "<rect x=\"{x:.3}\" y=\"{y:.3}\" width=\"{width:.3}\" height=\"{height:.3}\" fill=\"#eef2ff\" stroke=\"#3730a3\" stroke-width=\"0.7\"/>\n"
            ));
            out.push_str(&format!(
                "<text x=\"{:.3}\" y=\"{:.3}\" font-size=\"3\" fill=\"#111827\">{}</text>\n",
                x + 1.0,
                y + 4.0,
                xml_escape(&instance.name)
            ));
        }
        for instance in self.instances.values() {
            let Some(symbol) = self.symbols.get(&instance.symbol) else {
                continue;
            };
            for pin in &symbol.pins {
                let position = self.pin_position(&instance.name, &pin.name)?;
                out.push_str(&format!(
                    "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"0.8\" fill=\"#111827\"/>\n",
                    position.x, position.y
                ));
            }
        }
        for port in self.ports.values() {
            out.push_str(&format!(
                "<circle cx=\"{:.3}\" cy=\"{:.3}\" r=\"1.2\" fill=\"#dc2626\"/><text x=\"{:.3}\" y=\"{:.3}\" font-size=\"3\" fill=\"#dc2626\">{} [{}]</text>\n",
                port.position.x,
                port.position.y,
                port.position.x + 2.0,
                port.position.y,
                xml_escape(&port.name),
                port.direction.as_str()
            ));
        }
        out.push_str("</svg>\n");
        Ok(out)
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchematicEdit {
    AddInstance {
        name: String,
        symbol: String,
        transform: InstanceTransform,
    },
    MoveInstance {
        name: String,
        origin: Point,
    },
    SetInstanceTransform {
        name: String,
        rotation: Rotation,
        mirror_x: bool,
    },
    RemoveInstance {
        name: String,
    },
    AddWire {
        start: Point,
        end: Point,
    },
    RemoveWire {
        id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchematicEditor {
    document: SchematicDocument,
    undo: Vec<SchematicDocument>,
    redo: Vec<SchematicDocument>,
}

impl SchematicEditor {
    pub fn new(document: SchematicDocument) -> Result<Self, String> {
        document.validate()?;
        Ok(Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn document(&self) -> &SchematicDocument {
        &self.document
    }

    pub fn apply(&mut self, edit: SchematicEdit) -> Result<(), String> {
        let old = self.document.clone();
        match edit {
            SchematicEdit::AddInstance {
                name,
                symbol,
                transform,
            } => self.document.add_instance(name, &symbol, transform)?,
            SchematicEdit::MoveInstance { name, origin } => {
                self.document.move_instance(&name, origin)?
            }
            SchematicEdit::SetInstanceTransform {
                name,
                rotation,
                mirror_x,
            } => self
                .document
                .set_instance_transform(&name, rotation, mirror_x)?,
            SchematicEdit::RemoveInstance { name } => {
                self.document.remove_instance(&name)?;
            }
            SchematicEdit::AddWire { start, end } => {
                self.document.add_wire(start, end)?;
            }
            SchematicEdit::RemoveWire { id } => {
                self.document.remove_wire(&id)?;
            }
        }
        self.undo.push(old);
        self.redo.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(previous) = self.undo.pop() else {
            return Ok(false);
        };
        self.redo
            .push(std::mem::replace(&mut self.document, previous));
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        let Some(next) = self.redo.pop() else {
            return Ok(false);
        };
        self.undo.push(std::mem::replace(&mut self.document, next));
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchematicTool {
    Select,
    Wire,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchematicSelection {
    None,
    Pin { instance: String, pin: String },
    Port { name: String },
    Instance { name: String },
    Wire { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SchematicEvent {
    PointerDown { point: Point },
    PointerMove { point: Point },
    PointerUp { point: Point },
    SetTool { tool: SchematicTool },
    Undo,
    Redo,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SchematicEventOutcome {
    Ignored,
    SelectionChanged,
    Dragging { instance: String, origin: Point },
    InstanceMoved { instance: String, origin: Point },
    WirePreviewChanged { start: Point, end: Point },
    WireAdded { start: Point, end: Point },
    ToolChanged { tool: SchematicTool },
    UndoApplied { changed: bool },
    RedoApplied { changed: bool },
    Deleted { selection: SchematicSelection },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchematicSession {
    editor: SchematicEditor,
    tool: SchematicTool,
    selection: SchematicSelection,
    drag: Option<SchematicDrag>,
    wire_start: Option<Point>,
    wire_cursor: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SchematicDrag {
    instance_origin: Point,
    pointer_origin: Point,
    pointer_current: Point,
}

impl SchematicSession {
    pub fn new(document: SchematicDocument) -> Result<Self, String> {
        let editor = SchematicEditor::new(document)?;
        Ok(Self {
            editor,
            tool: SchematicTool::Select,
            selection: SchematicSelection::None,
            drag: None,
            wire_start: None,
            wire_cursor: Point::new(0.0, 0.0),
        })
    }

    pub fn document(&self) -> &SchematicDocument {
        self.editor.document()
    }

    pub fn tool(&self) -> SchematicTool {
        self.tool
    }

    pub fn selection(&self) -> &SchematicSelection {
        &self.selection
    }

    pub fn properties(&self) -> BTreeMap<String, String> {
        let mut properties = BTreeMap::new();
        properties.insert(
            "tool".into(),
            match self.tool {
                SchematicTool::Select => "select".into(),
                SchematicTool::Wire => "wire".into(),
            },
        );
        match &self.selection {
            SchematicSelection::None => {
                properties.insert("selection".into(), "none".into());
            }
            SchematicSelection::Pin { instance, pin } => {
                properties.insert("selection".into(), "pin".into());
                properties.insert("instance".into(), instance.clone());
                properties.insert("pin".into(), pin.clone());
                if let Ok(position) = self.editor.document().pin_position(instance, pin) {
                    properties.insert(
                        "position".into(),
                        format!("({:.3}, {:.3})", position.x, position.y),
                    );
                }
            }
            SchematicSelection::Port { name } => {
                properties.insert("selection".into(), "port".into());
                properties.insert("name".into(), name.clone());
                if let Some(port) = self.editor.document().ports.get(name) {
                    properties.insert("direction".into(), port.direction.as_str().into());
                    properties.insert(
                        "position".into(),
                        format!("({:.3}, {:.3})", port.position.x, port.position.y),
                    );
                }
            }
            SchematicSelection::Instance { name } => {
                properties.insert("selection".into(), "instance".into());
                properties.insert("name".into(), name.clone());
                if let Some(instance) = self.editor.document().instances.get(name) {
                    properties.insert("symbol".into(), instance.symbol.clone());
                    properties.insert(
                        "rotation".into(),
                        instance.transform.rotation.degrees().to_string(),
                    );
                    properties.insert("mirrorX".into(), instance.transform.mirror_x.to_string());
                    properties.insert(
                        "origin".into(),
                        format!(
                            "({:.3}, {:.3})",
                            instance.transform.origin.x, instance.transform.origin.y
                        ),
                    );
                }
            }
            SchematicSelection::Wire { id } => {
                properties.insert("selection".into(), "wire".into());
                properties.insert("id".into(), id.clone());
                if let Some(wire) = self.editor.document().wires.get(id) {
                    properties.insert(
                        "start".into(),
                        format!("({:.3}, {:.3})", wire.start.x, wire.start.y),
                    );
                    properties.insert(
                        "end".into(),
                        format!("({:.3}, {:.3})", wire.end.x, wire.end.y),
                    );
                }
            }
        }
        properties
    }

    pub fn render_svg(&self) -> Result<String, String> {
        self.document().write_svg()
    }

    pub fn process(&mut self, event: SchematicEvent) -> Result<SchematicEventOutcome, String> {
        match event {
            SchematicEvent::SetTool { tool } => {
                self.cancel_pending();
                let changed = self.tool != tool;
                self.tool = tool;
                if changed {
                    Ok(SchematicEventOutcome::ToolChanged { tool })
                } else {
                    Ok(SchematicEventOutcome::Ignored)
                }
            }
            SchematicEvent::PointerDown { point } => self.pointer_down(point),
            SchematicEvent::PointerMove { point } => self.pointer_move(point),
            SchematicEvent::PointerUp { point } => self.pointer_up(point),
            SchematicEvent::Undo => {
                self.cancel_pending();
                let changed = self.editor.undo()?;
                if changed {
                    self.selection = SchematicSelection::None;
                }
                Ok(SchematicEventOutcome::UndoApplied { changed })
            }
            SchematicEvent::Redo => {
                self.cancel_pending();
                let changed = self.editor.redo()?;
                if changed {
                    self.selection = SchematicSelection::None;
                }
                Ok(SchematicEventOutcome::RedoApplied { changed })
            }
            SchematicEvent::Delete => self.delete_selection(),
        }
    }

    fn cancel_pending(&mut self) {
        self.drag = None;
        self.wire_start = None;
    }

    fn pointer_down(&mut self, point: Point) -> Result<SchematicEventOutcome, String> {
        validate_session_point(point)?;
        self.cancel_pending();
        self.wire_cursor = point;
        let hit = self.editor.document().hit_test(point, 2.0)?;
        if self.tool == SchematicTool::Wire {
            let start = match &hit {
                Some(
                    SchematicHit::Pin { position, .. }
                    | SchematicHit::Port { position, .. }
                    | SchematicHit::Wire { position, .. },
                ) => *position,
                Some(SchematicHit::Instance { .. }) => point,
                None => point,
            };
            self.wire_start = Some(start);
            return Ok(SchematicEventOutcome::WirePreviewChanged { start, end: start });
        }

        let selection = match hit {
            Some(SchematicHit::Pin { instance, pin, .. }) => {
                SchematicSelection::Pin { instance, pin }
            }
            Some(SchematicHit::Port { name, .. }) => SchematicSelection::Port { name },
            Some(SchematicHit::Wire { id, .. }) => SchematicSelection::Wire { id },
            Some(SchematicHit::Instance { name }) => {
                let origin = self
                    .editor
                    .document()
                    .instances
                    .get(&name)
                    .map(|instance| instance.transform.origin)
                    .ok_or_else(|| format!("hit instance '{}' disappeared", name))?;
                self.drag = Some(SchematicDrag {
                    instance_origin: origin,
                    pointer_origin: point,
                    pointer_current: point,
                });
                SchematicSelection::Instance { name }
            }
            None => SchematicSelection::None,
        };
        let changed = selection != self.selection;
        self.selection = selection;
        if changed {
            Ok(SchematicEventOutcome::SelectionChanged)
        } else {
            Ok(SchematicEventOutcome::Ignored)
        }
    }

    fn pointer_move(&mut self, point: Point) -> Result<SchematicEventOutcome, String> {
        validate_session_point(point)?;
        self.wire_cursor = point;
        if let Some(start) = self.wire_start {
            return Ok(SchematicEventOutcome::WirePreviewChanged { start, end: point });
        }
        let Some(drag) = self.drag.as_mut() else {
            return Ok(SchematicEventOutcome::Ignored);
        };
        drag.pointer_current = point;
        if let SchematicSelection::Instance { name } = &self.selection {
            Ok(SchematicEventOutcome::Dragging {
                instance: name.clone(),
                origin: dragged_origin(*drag),
            })
        } else {
            Ok(SchematicEventOutcome::Ignored)
        }
    }

    fn pointer_up(&mut self, point: Point) -> Result<SchematicEventOutcome, String> {
        validate_session_point(point)?;
        if let Some(start) = self.wire_start.take() {
            self.wire_cursor = point;
            let hit = self.editor.document().hit_test(point, 2.0)?;
            let end = match &hit {
                Some(
                    SchematicHit::Pin { position, .. }
                    | SchematicHit::Port { position, .. }
                    | SchematicHit::Wire { position, .. },
                ) => *position,
                Some(SchematicHit::Instance { .. }) => point,
                None => point,
            };
            if start == point || start == end {
                return Ok(SchematicEventOutcome::Ignored);
            }
            self.editor.apply(SchematicEdit::AddWire { start, end })?;
            return Ok(SchematicEventOutcome::WireAdded { start, end });
        }
        let Some(drag) = self.drag.take() else {
            return Ok(SchematicEventOutcome::Ignored);
        };
        let mut drag = drag;
        drag.pointer_current = point;
        let origin = dragged_origin(drag);
        if let SchematicSelection::Instance { name } = &self.selection {
            let instance = name.clone();
            self.editor.apply(SchematicEdit::MoveInstance {
                name: instance.clone(),
                origin,
            })?;
            Ok(SchematicEventOutcome::InstanceMoved { instance, origin })
        } else {
            Ok(SchematicEventOutcome::Ignored)
        }
    }

    fn delete_selection(&mut self) -> Result<SchematicEventOutcome, String> {
        self.cancel_pending();
        let selection = self.selection.clone();
        match selection {
            SchematicSelection::Instance { ref name } => {
                self.editor
                    .apply(SchematicEdit::RemoveInstance { name: name.clone() })?;
            }
            SchematicSelection::Wire { ref id } => {
                self.editor
                    .apply(SchematicEdit::RemoveWire { id: id.clone() })?;
            }
            _ => return Ok(SchematicEventOutcome::Ignored),
        }
        self.selection = SchematicSelection::None;
        Ok(SchematicEventOutcome::Deleted { selection })
    }
}

fn validate_session_point(point: Point) -> Result<(), String> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err("schematic pointer coordinates must be finite".into())
    }
}

fn dragged_origin(drag: SchematicDrag) -> Point {
    Point::new(
        drag.instance_origin.x + drag.pointer_current.x - drag.pointer_origin.x,
        drag.instance_origin.y + drag.pointer_current.y - drag.pointer_origin.y,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resistor_symbol() -> Symbol {
        Symbol {
            name: "res".into(),
            width: 20.0,
            height: 8.0,
            pins: vec![
                SymbolPin {
                    name: "A".into(),
                    position: Point::new(0.0, 4.0),
                },
                SymbolPin {
                    name: "B".into(),
                    position: Point::new(20.0, 4.0),
                },
            ],
        }
    }

    fn document() -> SchematicDocument {
        let mut document = SchematicDocument::new("schematic & test");
        document.add_symbol(resistor_symbol()).unwrap();
        document
    }

    #[test]
    fn symbol_and_instance_validation_reject_duplicates() {
        let mut document = document();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        assert!(document
            .add_instance("R1", "res", InstanceTransform::default())
            .is_err());
        assert!(document
            .add_instance("R2", "missing", InstanceTransform::default())
            .is_err());
        let mut bad = resistor_symbol();
        bad.pins.push(bad.pins[0].clone());
        assert!(bad.validate().is_err());
    }

    #[test]
    fn rotation_and_mirror_transform_instance_pins() {
        let mut document = document();
        document
            .add_instance(
                "R1",
                "res",
                InstanceTransform {
                    origin: Point::new(30.0, 40.0),
                    rotation: Rotation::R90,
                    mirror_x: true,
                },
            )
            .unwrap();
        assert_eq!(
            document.pin_position("R1", "A").unwrap(),
            Point::new(34.0, 60.0)
        );
        assert_eq!(
            document.pin_position("R1", "B").unwrap(),
            Point::new(34.0, 40.0)
        );
    }

    #[test]
    fn wires_ports_and_pins_extract_into_named_nets() {
        let mut document = document();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        document
            .add_instance(
                "R2",
                "res",
                InstanceTransform {
                    origin: Point::new(50.0, 0.0),
                    ..Default::default()
                },
            )
            .unwrap();
        document
            .add_port("VIN", PortDirection::Input, Point::new(-5.0, 4.0))
            .unwrap();
        document
            .add_wire(Point::new(-5.0, 4.0), Point::new(0.0, 4.0))
            .unwrap();
        document
            .add_wire(Point::new(20.0, 4.0), Point::new(50.0, 4.0))
            .unwrap();
        document
            .add_wire(Point::new(70.0, 4.0), Point::new(90.0, 4.0))
            .unwrap();
        let nets = document.extract_nets().unwrap();
        assert_eq!(nets.len(), 3);
        let vin = nets.iter().find(|net| net.name == "VIN").unwrap();
        assert!(vin.port_aliases.contains(&"VIN".to_string()));
        assert!(vin.connections.contains(&SchematicConnection::Pin {
            instance: "R1".into(),
            pin: "A".into()
        }));
    }

    #[test]
    fn hit_test_prefers_exact_pins_within_radius() {
        let mut document = document();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        let hit = document
            .hit_test(Point::new(0.2, 4.0), 2.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            hit,
            SchematicHit::Pin {
                instance: "R1".into(),
                pin: "A".into(),
                position: Point::new(0.0, 4.0)
            }
        );
        assert!(document.hit_test(Point::new(0.0, 4.0), -1.0).is_err());
    }

    #[test]
    fn svg_escapes_user_text() {
        let mut document = document();
        document
            .add_instance("R<&>1", "res", InstanceTransform::default())
            .unwrap();
        let svg = document.write_svg().unwrap();
        assert!(svg.contains("&lt;&amp;&gt;"), "{svg}");
    }

    #[test]
    fn editor_undo_and_redo_atomic_edits() {
        let mut editor = SchematicEditor::new(document()).unwrap();
        editor
            .apply(SchematicEdit::AddInstance {
                name: "R1".into(),
                symbol: "res".into(),
                transform: InstanceTransform::default(),
            })
            .unwrap();
        assert!(editor.document().instances.contains_key("R1"));
        editor
            .apply(SchematicEdit::RemoveInstance { name: "R1".into() })
            .unwrap();
        assert!(!editor.document().instances.contains_key("R1"));
        assert!(editor.undo().unwrap());
        assert!(editor.document().instances.contains_key("R1"));
        assert!(editor.undo().unwrap());
        assert!(!editor.document().instances.contains_key("R1"));
        assert!(editor.redo().unwrap());
        assert!(editor.document().instances.contains_key("R1"));
    }

    #[test]
    fn interactive_session_drags_and_undoes_instances() {
        let mut document = document();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        let mut session = SchematicSession::new(document).unwrap();
        assert_eq!(session.tool(), SchematicTool::Select);
        assert_eq!(
            session
                .process(SchematicEvent::PointerDown {
                    point: Point::new(10.0, 4.0),
                })
                .unwrap(),
            SchematicEventOutcome::SelectionChanged
        );
        assert_eq!(
            session.selection(),
            &SchematicSelection::Instance { name: "R1".into() }
        );
        assert_eq!(
            session
                .process(SchematicEvent::PointerMove {
                    point: Point::new(13.0, 7.0),
                })
                .unwrap(),
            SchematicEventOutcome::Dragging {
                instance: "R1".into(),
                origin: Point::new(3.0, 3.0),
            }
        );
        assert_eq!(
            session
                .process(SchematicEvent::PointerUp {
                    point: Point::new(13.0, 7.0),
                })
                .unwrap(),
            SchematicEventOutcome::InstanceMoved {
                instance: "R1".into(),
                origin: Point::new(3.0, 3.0),
            }
        );
        assert_eq!(
            session.document().instances["R1"].transform.origin,
            Point::new(3.0, 3.0)
        );
        assert_eq!(
            session.process(SchematicEvent::Undo).unwrap(),
            SchematicEventOutcome::UndoApplied { changed: true }
        );
        assert_eq!(
            session.document().instances["R1"].transform.origin,
            Point::new(0.0, 0.0)
        );
    }

    #[test]
    fn interactive_wire_tool_connects_and_deletes_wires() {
        let mut document = document();
        document
            .add_instance("R1", "res", InstanceTransform::default())
            .unwrap();
        let mut session = SchematicSession::new(document).unwrap();
        session
            .process(SchematicEvent::SetTool {
                tool: SchematicTool::Wire,
            })
            .unwrap();
        assert_eq!(
            session
                .process(SchematicEvent::PointerDown {
                    point: Point::new(0.1, 4.0),
                })
                .unwrap(),
            SchematicEventOutcome::WirePreviewChanged {
                start: Point::new(0.0, 4.0),
                end: Point::new(0.0, 4.0),
            }
        );
        assert_eq!(
            session
                .process(SchematicEvent::PointerUp {
                    point: Point::new(20.1, 4.0),
                })
                .unwrap(),
            SchematicEventOutcome::WireAdded {
                start: Point::new(0.0, 4.0),
                end: Point::new(20.0, 4.0),
            }
        );
        let nets = session.document().extract_nets().unwrap();
        assert_eq!(nets.len(), 1);
        session
            .process(SchematicEvent::SetTool {
                tool: SchematicTool::Select,
            })
            .unwrap();
        session
            .process(SchematicEvent::PointerDown {
                point: Point::new(10.0, 4.0),
            })
            .unwrap();
        assert_eq!(
            session.selection(),
            &SchematicSelection::Wire { id: "W1".into() }
        );
        assert_eq!(
            session.process(SchematicEvent::Delete).unwrap(),
            SchematicEventOutcome::Deleted {
                selection: SchematicSelection::Wire { id: "W1".into() }
            }
        );
        assert!(session.document().wires.is_empty());
    }

    #[test]
    fn interactive_session_rejects_nonfinite_pointer_coordinates() {
        let session = SchematicSession::new(document()).unwrap();
        let mut session = session;
        assert!(session
            .process(SchematicEvent::PointerDown {
                point: Point::new(f64::NAN, 0.0),
            })
            .is_err());
    }
}
