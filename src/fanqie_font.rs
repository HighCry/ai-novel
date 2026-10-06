//! 番茄加密字体自动解码：番茄换了字体时，把新字体和内置的旧字体逐个比字形，形状对得上的直接认出来，
//! 不用再一页页打开书页学。内置的旧字体就是 fanqie_glyphs.json 对照表的那个字体。
//! 番茄的字形取自思源黑体，同一个字在新旧字体里形状基本一样；拿不准的宁可不认，留给书页去学。

use crate::trends::is_pua;
use ab_glyph::{point, Font, FontVec, Outline, OutlineCurve, Point};
use ab_glyph_rasterizer::Rasterizer;
use anyhow::Context;
use std::collections::BTreeMap;

/// fanqie_glyphs.json 对照表对应的字体文件
const KNOWN_FONT: &[u8] = include_bytes!("fanqie_font.woff2");
/// 字形按自己的外框等比缩放，画成 SIDE×SIDE 的灰度再比
const SIDE: usize = 32;
/// 认定是同一个字：和它的差别不到「它和最像的别的字之间差别」的这个比例。
/// 同一个字换成思源黑体原版字形，比例最多 0.22；GB2312 一级字里的形近字（土/士、曰/口、井/并……）最少也有 0.35
const SAME: f32 = 0.25;
/// 并且最像的要明显比第二像的近
const MARGIN: f32 = 0.6;

type Shape = Vec<f32>;

/// 字体里每个私用区字符的字形
fn shapes(woff2: &[u8]) -> anyhow::Result<BTreeMap<u32, Shape>> {
    let sfnt = wuff::decompress_woff2(woff2).context("番茄字体文件解不开")?;
    let font = FontVec::try_from_vec(sfnt).context("番茄字体文件读不了")?;
    Ok(font.codepoint_ids().filter(|(_, c)| is_pua(*c)).filter_map(|(id, c)| Some((c as u32, raster(&font.outline(id)?)))).collect())
}

/// 字形按外框等比缩放、居中，画成 SIDE×SIDE 的灰度；四周留半格，免得画到边外
fn raster(o: &Outline) -> Shape {
    let b = o.bounds;
    let (x0, x1, y0, y1) = (b.min.x.min(b.max.x), b.min.x.max(b.max.x), b.min.y.min(b.max.y), b.min.y.max(b.max.y));
    let side = SIDE as f32;
    let k = (side - 1.0) / (x1 - x0).max(y1 - y0).max(1.0);
    let (dx, dy) = ((side - (x1 - x0) * k) / 2.0, (side - (y1 - y0) * k) / 2.0);
    let at = |p: Point| point((p.x - x0) * k + dx, (y1 - p.y) * k + dy);
    let mut r = Rasterizer::new(SIDE, SIDE);
    for c in &o.curves {
        match *c {
            OutlineCurve::Line(a, b) => r.draw_line(at(a), at(b)),
            OutlineCurve::Quad(a, b, c) => r.draw_quad(at(a), at(b), at(c)),
            OutlineCurve::Cubic(a, b, c, d) => r.draw_cubic(at(a), at(b), at(c), at(d)),
        }
    }
    let mut px = vec![0.0; SIDE * SIDE];
    r.for_each_pixel(|i, v| px[i] = v.min(1.0));
    px
}

fn dist(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()
}

/// 和最像的字差 best、和第二像的差 second，最像的那个字和别的字最少差 floor：能不能认定
fn confident(best: f32, second: f32, floor: f32) -> bool {
    best <= SAME * floor && best < MARGIN * second
}

/// 认得的字形：每个字、它的字形，和它与最像的别的字差多少
struct Known {
    chars: Vec<char>,
    shapes: Vec<Shape>,
    floor: Vec<f32>,
}

impl Known {
    fn new(font: &BTreeMap<u32, Shape>, map: &BTreeMap<u32, char>) -> Self {
        let (chars, shapes): (Vec<char>, Vec<Shape>) = font.iter().filter_map(|(k, s)| Some((*map.get(k)?, s.clone()))).unzip();
        let mut floor = vec![f32::INFINITY; shapes.len()];
        for i in 0..shapes.len() {
            for j in i + 1..shapes.len() {
                let d = dist(&shapes[i], &shapes[j]);
                floor[i] = floor[i].min(d);
                floor[j] = floor[j].min(d);
            }
        }
        Known { chars, shapes, floor }
    }

    /// 最像的字，能认定才返回 (第几个字, 差多少)
    fn pick(&self, s: &[f32]) -> Option<(usize, f32)> {
        let (mut best, mut second) = ((usize::MAX, f32::INFINITY), f32::INFINITY);
        for (i, k) in self.shapes.iter().enumerate() {
            let d = dist(s, k);
            if d < best.1 {
                second = best.1;
                best = (i, d);
            } else if d < second {
                second = d;
            }
        }
        (best.0 != usize::MAX && confident(best.1, second, self.floor[best.0])).then_some(best)
    }
}

/// 新字体里每个字形找最像的认得的字；两个字形认成同一个字时只留更像的那个
fn recognize(known: &Known, font: &BTreeMap<u32, Shape>) -> BTreeMap<u32, char> {
    let mut by_char: BTreeMap<usize, (u32, f32)> = BTreeMap::new();
    for (code, s) in font {
        if let Some((i, d)) = known.pick(s) {
            let e = by_char.entry(i).or_insert((*code, d));
            if d < e.1 {
                *e = (*code, d);
            }
        }
    }
    by_char.into_iter().map(|(i, (code, _))| (code, known.chars[i])).collect()
}

/// 拿内置字体认番茄的新字体，返回认出来的「私用区编码 → 字」；map 是内置字体的对照表
pub fn recognize_font(woff2: &[u8], map: &BTreeMap<u32, char>) -> anyhow::Result<BTreeMap<u32, char>> {
    let known = Known::new(&shapes(KNOWN_FONT)?, map);
    Ok(recognize(&known, &shapes(woff2)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内置字体的私用区编码整体错开 7 位、每个轮廓点随机挪动 ±12（字号 1000），模拟番茄换了字体
    const SHUFFLED: &[u8] = include_bytes!("../tests/data/fanqie_shuffled.woff2");
    const SHIFT: usize = 7;

    fn seed() -> &'static BTreeMap<u32, char> {
        &crate::trends::seed_glyphs().expect("内置对照表能解析").map
    }

    #[test]
    fn recognizes_its_own_font() {
        assert_eq!(&recognize_font(KNOWN_FONT, seed()).unwrap(), seed(), "内置字体和内置对照表是同一个字体，362 个字都要认对");
        assert!(recognize_font(b"not a font", seed()).is_err());
    }

    #[test]
    fn follows_a_reshuffled_and_redrawn_font() {
        let codes: Vec<u32> = seed().keys().copied().collect();
        let want = |i: usize| seed()[&codes[(i + SHIFT) % codes.len()]];
        let found = recognize_font(SHUFFLED, seed()).unwrap();
        let wrong: Vec<String> = codes.iter().enumerate().filter_map(|(i, c)| found.get(c).filter(|v| **v != want(i)).map(|v| format!("{c:x}:{v}≠{}", want(i)))).collect();
        assert!(wrong.is_empty(), "认出来的必须都对：{wrong:?}");
        assert!(found.len() * 2 > codes.len(), "编码打乱、字形挪动过也要认出一大半：{}/{}", found.len(), codes.len());
    }

    /// 把每个字轮流当成内置字体里没有的字：都不能被认成别的字（土/士、已/己 这类形近字最容易错）
    #[test]
    fn never_names_a_char_it_does_not_know() {
        let ks = shapes(KNOWN_FONT).unwrap();
        let known = Known::new(&ks, seed());
        let n = known.shapes.len();
        let d: Vec<Vec<f32>> = (0..n).map(|i| (0..n).map(|j| if i == j { f32::INFINITY } else { dist(&known.shapes[i], &known.shapes[j]) }).collect()).collect();
        let mut wrong = Vec::new();
        for x in 0..n {
            let mut order: Vec<usize> = (0..n).filter(|j| *j != x).collect();
            order.sort_by(|a, b| d[x][*a].total_cmp(&d[x][*b]));
            let (best, second) = (order[0], order[1]);
            let floor = (0..n).filter(|k| *k != x).map(|k| d[best][k]).fold(f32::INFINITY, f32::min);
            if confident(d[x][best], d[x][second], floor) {
                wrong.push(format!("{}→{}", known.chars[x], known.chars[best]));
            }
        }
        assert!(wrong.is_empty(), "{wrong:?}");
    }
}
