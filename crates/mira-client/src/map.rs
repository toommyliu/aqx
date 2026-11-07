use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};

const MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Default, PartialEq)]
pub(crate) struct MapFile {
    pub(crate) cells: Vec<Cell>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Cell {
    pub(crate) name: String,
    pub(crate) pads: Vec<Pad>,
    pub(crate) doors: Vec<Door>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Pad {
    pub(crate) name: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Door {
    pub(crate) id: i64,
    pub(crate) cell: String,
    pub(crate) pad: String,
}

impl MapFile {
    pub(crate) fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|cell| cell.name == name)
    }
}

pub(crate) fn read(bytes: &[u8]) -> io::Result<MapFile> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid map file");
    let header = bytes.get(..8).ok_or_else(invalid)?;
    let body = match &header[..3] {
        b"FWS" => bytes[8..].to_vec(),
        b"CWS" => {
            let mut body = Vec::new();
            flate2::read::ZlibDecoder::new(&bytes[8..])
                .take(MAX_BYTES + 1)
                .read_to_end(&mut body)?;
            if body.len() as u64 > MAX_BYTES {
                return Err(invalid());
            }
            body
        }
        _ => return Err(invalid()),
    };
    timeline(&body).ok_or_else(invalid)
}

struct Placed {
    character: u16,
    name: Option<String>,
    x: i32,
    y: i32,
}

struct Labeled {
    frame: usize,
    name: String,
    stage: Vec<(String, u16, i32, i32)>,
}

fn timeline(body: &[u8]) -> Option<MapFile> {
    let bits = body.first()? >> 3;
    let mut r = Reader::at(body, (5 + 4 * bits as usize).div_ceil(8) + 4);
    let mut frame = 0;
    let mut label = None;
    let mut labeled: Vec<Labeled> = Vec::new();
    let mut stage: BTreeMap<u16, Placed> = BTreeMap::new();
    let mut classes: HashMap<u16, String> = HashMap::new();
    let mut setters = Vec::new();
    loop {
        let tag = r.u16()?;
        let mut len = (tag & 63) as usize;
        if len == 63 {
            len = r.u32()? as usize;
        }
        let data = r.bytes(len)?;
        match tag >> 6 {
            0 => break,
            1 => {
                if let Some(name) = label.take()
                    && !labeled.iter().any(|l| l.name == name)
                {
                    let stage = stage
                        .values()
                        .filter_map(|p| Some((p.name.clone()?, p.character, p.x, p.y)))
                        .collect();
                    labeled.push(Labeled { frame, name, stage });
                }
                frame += 1;
            }
            43 => {
                let name = Reader::at(data, 0).cstr()?;
                if !name.is_empty() {
                    label = Some(name);
                }
            }
            26 | 70 => place(&mut stage, data, tag >> 6 == 70)?,
            5 => {
                stage.remove(&Reader::at(data, 2).u16()?);
            }
            28 => {
                stage.remove(&Reader::at(data, 0).u16()?);
            }
            76 => {
                let mut r = Reader::at(data, 0);
                for _ in 0..r.u16()? {
                    let id = r.u16()?;
                    classes.insert(id, r.cstr()?);
                }
            }
            72 => setters.extend(abc::setters(data).unwrap_or_default()),
            82 => {
                let mut r = Reader::at(data, 4);
                r.cstr()?;
                setters.extend(abc::setters(&data[r.at..]).unwrap_or_default());
            }
            _ => {}
        }
    }
    let is_pad = |character: &u16| {
        classes
            .get(character)
            .and_then(|class| class.rsplit_once(".Pad_"))
            .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let document = classes.get(&0);
    let mut cells: Vec<Cell> = labeled
        .iter()
        .map(|l| Cell {
            name: l.name.clone(),
            pads: l
                .stage
                .iter()
                .filter(|(_, character, _, _)| is_pad(character))
                .map(|(name, _, x, y)| Pad {
                    name: name.clone(),
                    x: *x,
                    y: *y,
                })
                .collect(),
            doors: vec![],
        })
        .collect();
    for setter in setters.into_iter().filter(|s| Some(&s.class) == document) {
        let Some(at) = labeled.iter().rposition(|l| l.frame < setter.frame) else {
            continue;
        };
        cells[at].doors.extend(setter.door());
    }
    Some(MapFile { cells })
}

fn place(stage: &mut BTreeMap<u16, Placed>, data: &[u8], three: bool) -> Option<()> {
    let mut r = Reader::at(data, 0);
    let flags = r.u8()?;
    let flags2 = if three { r.u8()? } else { 0 };
    let depth = r.u16()?;
    if flags2 & 8 != 0 || (flags2 & 16 != 0 && flags & 2 != 0) {
        r.cstr()?;
    }
    let character = if flags & 2 != 0 { Some(r.u16()?) } else { None };
    let position = if flags & 4 != 0 {
        Some(r.translation()?)
    } else {
        None
    };
    if flags & 8 != 0 {
        r.color_transform()?;
    }
    if flags & 16 != 0 {
        r.u16()?;
    }
    let name = if flags & 32 != 0 {
        Some(r.cstr()?)
    } else {
        None
    };
    match stage.get_mut(&depth) {
        Some(placed) if flags & 1 != 0 => {
            placed.character = character.unwrap_or(placed.character);
            (placed.x, placed.y) = position.unwrap_or((placed.x, placed.y));
            placed.name = name.or(placed.name.take());
        }
        _ => {
            if let Some(character) = character {
                let (x, y) = position.unwrap_or_default();
                stage.insert(
                    depth,
                    Placed {
                        character,
                        name,
                        x,
                        y,
                    },
                );
            }
        }
    }
    Some(())
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn at(data: &'a [u8], at: usize) -> Self {
        Reader { data, at }
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.bytes(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }

    fn u30(&mut self) -> Option<u32> {
        let mut value = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= u32::from(byte & 127) << shift;
            if byte & 128 == 0 {
                break;
            }
        }
        Some(value)
    }

    fn cstr(&mut self) -> Option<String> {
        let rest = self.data.get(self.at..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        self.at += end + 1;
        String::from_utf8(rest[..end].to_vec()).ok()
    }

    fn translation(&mut self) -> Option<(i32, i32)> {
        let mut bits = Bits::at(self);
        if bits.read(1)? == 1 {
            let n = bits.read(5)?;
            bits.read(2 * n)?;
        }
        if bits.read(1)? == 1 {
            let n = bits.read(5)?;
            bits.read(2 * n)?;
        }
        let n = bits.read(5)?;
        let x = bits.signed(n)?;
        let y = bits.signed(n)?;
        self.at = bits.end();
        Some((x / 20, y / 20))
    }

    fn color_transform(&mut self) -> Option<()> {
        let mut bits = Bits::at(self);
        let add = bits.read(1)?;
        let multiply = bits.read(1)?;
        let n = bits.read(4)?;
        bits.read(4 * n * (add + multiply))?;
        self.at = bits.end();
        Some(())
    }
}

struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> Bits<'a> {
    fn at(r: &Reader<'a>) -> Self {
        Bits {
            data: r.data,
            bit: r.at * 8,
        }
    }

    fn read(&mut self, n: u32) -> Option<u32> {
        let mut value: u64 = 0;
        for _ in 0..n {
            let byte = self.data.get(self.bit / 8)?;
            value = (value << 1) | u64::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        u32::try_from(value & 0xffff_ffff).ok()
    }

    fn signed(&mut self, n: u32) -> Option<i32> {
        let value = self.read(n)?;
        Some(if n > 0 && n < 32 && value >> (n - 1) & 1 == 1 {
            (value as i64 - (1i64 << n)) as i32
        } else {
            value as i32
        })
    }

    fn end(&self) -> usize {
        self.bit.div_ceil(8)
    }
}

mod abc {
    use super::{Door, Reader};

    pub(super) struct Setter {
        pub(super) class: String,
        pub(super) frame: usize,
        target: Option<String>,
        cell: Option<String>,
        pad: Option<String>,
        id: Option<i64>,
    }

    impl Setter {
        fn set(&mut self, target: &str, property: &str, value: Option<Value>) {
            self.target = Some(target.to_owned());
            match (property, value) {
                ("tCell", Some(Value::Str(cell))) => self.cell = Some(cell.to_owned()),
                ("tPad", Some(Value::Str(pad))) => self.pad = Some(pad.to_owned()),
                ("tID", Some(Value::Num(id))) if id.fract() == 0.0 => self.id = Some(id as i64),
                ("tID", Some(Value::Str(id))) => self.id = id.parse().ok(),
                _ => {}
            }
        }

        pub(super) fn door(self) -> Option<Door> {
            let id = self
                .id
                .or_else(|| self.target.as_deref()?.strip_prefix("ia")?.parse().ok())?;
            Some(Door {
                id,
                cell: self.cell?,
                pad: self.pad?,
            })
        }
    }

    #[derive(Clone, Copy)]
    enum Value<'a> {
        Str(&'a str),
        Num(f64),
    }

    #[derive(Clone, Default)]
    struct Name {
        space: Option<u32>,
        local: Option<String>,
    }

    struct Pool {
        ints: Vec<f64>,
        uints: Vec<f64>,
        doubles: Vec<f64>,
        strings: Vec<String>,
        spaces: Vec<String>,
        names: Vec<Name>,
    }

    impl Pool {
        fn string(&self, i: u32) -> Option<&str> {
            self.strings.get(i as usize).map(String::as_str)
        }

        fn local(&self, i: u32) -> Option<&str> {
            self.names.get(i as usize)?.local.as_deref()
        }

        fn qualified(&self, i: u32) -> Option<String> {
            let name = self.names.get(i as usize)?;
            let local = name.local.as_deref()?;
            let space = self.spaces.get(name.space? as usize)?;
            Some(if space.is_empty() {
                local.to_owned()
            } else {
                format!("{space}.{local}")
            })
        }
    }

    pub(super) fn setters(data: &[u8]) -> Option<Vec<Setter>> {
        let mut r = Reader::at(data, 4);
        let pool = pool(&mut r)?;
        for _ in 0..r.u30()? {
            let params = r.u30()?;
            r.u30()?;
            for _ in 0..params {
                r.u30()?;
            }
            r.u30()?;
            let flags = r.u8()?;
            if flags & 8 != 0 {
                for _ in 0..r.u30()? {
                    r.u30()?;
                    r.u8()?;
                }
            }
            if flags & 128 != 0 {
                for _ in 0..params {
                    r.u30()?;
                }
            }
        }
        for _ in 0..r.u30()? {
            r.u30()?;
            for _ in 0..r.u30()? {
                r.u30()?;
                r.u30()?;
            }
        }
        let classes = r.u30()?;
        let mut methods = Vec::new();
        for _ in 0..classes {
            let class = pool.qualified(r.u30()?);
            r.u30()?;
            if r.u8()? & 8 != 0 {
                r.u30()?;
            }
            for _ in 0..r.u30()? {
                r.u30()?;
            }
            r.u30()?;
            for (name, method) in traits(&mut r, &pool)? {
                if let Some(class) = &class
                    && name.starts_with("__setProp_")
                {
                    methods.push((class.clone(), name, method));
                }
            }
        }
        for _ in 0..classes {
            r.u30()?;
            traits(&mut r, &pool)?;
        }
        for _ in 0..r.u30()? {
            r.u30()?;
            traits(&mut r, &pool)?;
        }
        let mut bodies = std::collections::HashMap::new();
        for _ in 0..r.u30()? {
            let method = r.u30()?;
            for _ in 0..4 {
                r.u30()?;
            }
            let len = r.u30()? as usize;
            bodies.insert(method, r.bytes(len)?);
            for _ in 0..r.u30()? {
                for _ in 0..5 {
                    r.u30()?;
                }
            }
            traits(&mut r, &pool)?;
        }
        Some(
            methods
                .into_iter()
                .filter_map(|(class, name, method)| {
                    let frame = name.rsplit('_').next()?.parse().ok()?;
                    let mut setter = Setter {
                        class,
                        frame,
                        target: None,
                        cell: None,
                        pad: None,
                        id: None,
                    };
                    assign(&mut setter, bodies.get(&method)?, &pool);
                    Some(setter)
                })
                .collect(),
        )
    }

    fn pool(r: &mut Reader) -> Option<Pool> {
        let mut ints = vec![0.0];
        for _ in 1..r.u30()?.max(1) {
            ints.push(f64::from(r.u30()? as i32));
        }
        let mut uints = vec![0.0];
        for _ in 1..r.u30()?.max(1) {
            uints.push(f64::from(r.u30()?));
        }
        let mut doubles = vec![0.0];
        for _ in 1..r.u30()?.max(1) {
            doubles.push(r.f64()?);
        }
        let mut strings = vec![String::new()];
        for _ in 1..r.u30()?.max(1) {
            let len = r.u30()? as usize;
            strings.push(String::from_utf8_lossy(r.bytes(len)?).into_owned());
        }
        let mut spaces = vec![String::new()];
        for _ in 1..r.u30()?.max(1) {
            r.u8()?;
            spaces.push(strings.get(r.u30()? as usize)?.clone());
        }
        for _ in 1..r.u30()?.max(1) {
            for _ in 0..r.u30()? {
                r.u30()?;
            }
        }
        let mut names = vec![Name::default()];
        for _ in 1..r.u30()?.max(1) {
            let local = |r: &mut Reader| Some(strings.get(r.u30()? as usize)?.clone());
            names.push(match r.u8()? {
                7 | 13 => {
                    let space = r.u30()?;
                    Name {
                        space: Some(space),
                        local: local(r)?.into(),
                    }
                }
                15 | 16 => Name {
                    space: None,
                    local: local(r)?.into(),
                },
                9 | 14 => {
                    let local = local(r)?;
                    r.u30()?;
                    Name {
                        space: None,
                        local: Some(local),
                    }
                }
                27 | 28 => {
                    r.u30()?;
                    Name::default()
                }
                17 | 18 => Name::default(),
                29 => {
                    r.u30()?;
                    for _ in 0..r.u30()? {
                        r.u30()?;
                    }
                    Name::default()
                }
                _ => return None,
            });
        }
        Some(Pool {
            ints,
            uints,
            doubles,
            strings,
            spaces,
            names,
        })
    }

    fn traits(r: &mut Reader, pool: &Pool) -> Option<Vec<(String, u32)>> {
        let mut methods = Vec::new();
        for _ in 0..r.u30()? {
            let name = pool.local(r.u30()?).map(str::to_owned);
            let kind = r.u8()?;
            match kind & 15 {
                0 | 6 => {
                    r.u30()?;
                    r.u30()?;
                    if r.u30()? != 0 {
                        r.u8()?;
                    }
                }
                1..=3 => {
                    r.u30()?;
                    let method = r.u30()?;
                    methods.extend(name.map(|name| (name, method)));
                }
                4 | 5 => {
                    r.u30()?;
                    r.u30()?;
                }
                _ => return None,
            }
            if kind & 0x40 != 0 {
                for _ in 0..r.u30()? {
                    r.u30()?;
                }
            }
        }
        Some(methods)
    }

    const BARE: &[u8] = &[
        0x02, 0x09, 0x1d, 0x29, 0x2a, 0x2b, 0x30, 0x47, 0x48, 0x57, 0x64, 0x73, 0x82, 0x85, 0xd0,
        0xd1, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7,
    ];
    const INDEXED: &[u8] = &[
        0x08, 0x5a, 0x5d, 0x5e, 0x62, 0x63, 0x6c, 0x6d, 0x80, 0xf0, 0xf1,
    ];

    fn assign(setter: &mut Setter, code: &[u8], pool: &Pool) {
        let mut r = Reader::at(code, 0);
        let mut target = None;
        let mut value = None;
        let number = |numbers: &[f64], i: u32| numbers.get(i as usize).copied().map(Value::Num);
        while let Some(op) = r.u8() {
            let step = match op {
                0x60 | 0x66 => r.u30().map(|i| target = pool.local(i)),
                0x2c => r.u30().map(|i| value = pool.string(i).map(Value::Str)),
                0x24 => r.u8().map(|b| value = Some(Value::Num(f64::from(b as i8)))),
                0x25 => r
                    .u30()
                    .map(|v| value = Some(Value::Num(f64::from(v as u16 as i16)))),
                0x2d => r.u30().map(|i| value = number(&pool.ints, i)),
                0x2e => r.u30().map(|i| value = number(&pool.uints, i)),
                0x2f => r.u30().map(|i| value = number(&pool.doubles, i)),
                0x20 | 0x21 | 0x26 | 0x27 | 0x28 => {
                    value = None;
                    Some(())
                }
                0x61 | 0x68 => r.u30().map(|i| {
                    if let (Some(name), Some(on)) = (pool.local(i), target) {
                        setter.set(on, name, value);
                    }
                }),
                op if BARE.contains(&op) => Some(()),
                op if INDEXED.contains(&op) => r.u30().map(drop),
                0x65 => r.u8().map(drop),
                0x0c..=0x1a => r.bytes(3).map(drop),
                0xef => r.u8().and(r.u30()).and(r.u8()).and(r.u30()).map(drop),
                _ => None,
            };
            if step.is_none() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{MapCell, map_swf, map_swf_with};

    #[test]
    fn map_cells_come_from_frame_labels() {
        let bytes = map_swf(&["Enter", "Side1", "Blank", "Enter"]);
        let names = |map: MapFile| map.cells.into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(read(&bytes).unwrap()), ["Enter", "Side1", "Blank"]);
        use std::io::Write;
        let mut encoded =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoded.write_all(&bytes[8..]).unwrap();
        let compressed = [b"CWS\x0a\0\0\0\0".as_slice(), &encoded.finish().unwrap()].concat();
        assert_eq!(
            names(read(&compressed).unwrap()),
            ["Enter", "Side1", "Blank"]
        );
        assert!(read(b"FWS").is_err());
        assert!(read(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn map_cells_list_their_pads_and_doors() {
        let bytes = map_swf_with(&[
            MapCell {
                name: "Enter",
                pads: &[("Spawn", 322, 350), ("Right", 788, 350)],
                doors: &[(5, "r2", "Left"), (6, "r3", "Top")],
            },
            MapCell {
                name: "r2",
                pads: &[("Left", 190, 350)],
                doors: &[(7, "Enter", "Right")],
            },
            MapCell::named("Blank"),
        ]);
        let map = read(&bytes).unwrap();
        let pad = |name: &str, x, y| Pad {
            name: name.into(),
            x,
            y,
        };
        let door = |id, cell: &str, pad: &str| Door {
            id,
            cell: cell.into(),
            pad: pad.into(),
        };
        assert_eq!(
            map.cells,
            [
                Cell {
                    name: "Enter".into(),
                    pads: vec![pad("Spawn", 322, 350), pad("Right", 788, 350)],
                    doors: vec![door(5, "r2", "Left"), door(6, "r3", "Top")],
                },
                Cell {
                    name: "r2".into(),
                    pads: vec![pad("Left", 190, 350)],
                    doors: vec![door(7, "Enter", "Right")],
                },
                Cell {
                    name: "Blank".into(),
                    pads: vec![],
                    doors: vec![],
                },
            ]
        );
    }
}
