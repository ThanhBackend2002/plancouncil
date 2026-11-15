// Bộ sinh nhân vật pixel-art 16×24 từ thông tin role — không dùng asset ngoài.
// Nhân vật = nhiều lớp template (1 ký tự = 1 pixel, '.' = trong suốt) vẽ chồng lên nhau:
//   thân (da) → mặt (nam/nữ) → trang phục → chân (quần/váy) → tóc → kính → phụ kiện.
// Mọi thuộc tính ngoại hình để trống sẽ được tự sinh ổn định từ id + cấp bậc.

import type { Appearance, RoleLevel } from "./contracts";

export const SPRITE_W = 16;
export const SPRITE_H = 24;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
  fill: string;
}

export interface Sprite {
  body: Rect[];
  /** Miệng mở (đè lên body khi đang nói). */
  mouthOpen: Rect[];
  /** Mắt nhắm (chớp mắt). Rỗng nếu đeo kính râm. */
  blink: Rect[];
}

type Layer = string[];
const blank = (n: number): Layer => Array<string>(n).fill("................");
const at = (row: number, rows: Layer): Layer => [...blank(row), ...rows];

// ───────────────────────────── Bảng màu ─────────────────────────────

export const SKINS: { id: string; label: string; color: string }[] = [
  { id: "porcelain", label: "Rất sáng", color: "#ffe3cc" },
  { id: "light", label: "Sáng", color: "#f8d0a8" },
  { id: "beige", label: "Be", color: "#f1c27d" },
  { id: "tan", label: "Rám", color: "#e0ac69" },
  { id: "olive", label: "Ô liu", color: "#c99a62" },
  { id: "brown", label: "Nâu", color: "#a86b3c" },
  { id: "dark", label: "Nâu đậm", color: "#7a4a2a" },
  { id: "deep", label: "Đậm", color: "#55301c" },
];

export const HAIR_COLORS: { id: string; label: string; color: string; natural: boolean }[] = [
  { id: "black", label: "Đen", color: "#1c1917", natural: true },
  { id: "dark_brown", label: "Nâu đậm", color: "#3b2416", natural: true },
  { id: "brown", label: "Nâu", color: "#6b4226", natural: true },
  { id: "auburn", label: "Nâu đỏ", color: "#8b3a1a", natural: true },
  { id: "blonde", label: "Vàng", color: "#e2b84d", natural: true },
  { id: "platinum", label: "Bạch kim", color: "#ece3c8", natural: false },
  { id: "gray", label: "Muối tiêu", color: "#9ca3af", natural: true },
  { id: "white", label: "Bạc", color: "#e5e7eb", natural: false },
  { id: "red", label: "Đỏ", color: "#c2410c", natural: false },
  { id: "pink", label: "Hồng", color: "#f472b6", natural: false },
  { id: "purple", label: "Tím", color: "#8b5cf6", natural: false },
  { id: "blue", label: "Xanh dương", color: "#3b82f6", natural: false },
  { id: "green", label: "Xanh lá", color: "#10b981", natural: false },
];

// ───────────────────────────── Thân + mặt ─────────────────────────────

const BODY: Layer = [
  "................",
  "................",
  "....SSSSSSSS....",
  "...SSSSSSSSSS...",
  "..SSSSSSSSSSSS..",
  "..SSSSSSSSSSSS..",
  "..SSSSSSSSSSSS..",
  "..SSSSSSSSSSSS..",
  "..SSSSSSSSSSSS..",
  "...SSSSSSSSSS...",
  "....SSSSSSSS....",
  "......SSSS......",
];

const FACE_MALE: Layer = at(6, [
  "....EE....EE....",
  "....EE....EE....",
  "................",
  ".......MM.......",
]);

const FACE_FEMALE: Layer = at(5, [
  "...E........E...",
  "....EE....EE....",
  "....EE....EE....",
  "...R........R...",
  "......MMMM......",
]);

const MOUTH_OPEN: Layer = at(9, ["......MMMM......", ".......MM......."]);
const BLINK: Layer = at(6, ["....SS....SS...."]);

// ───────────────────────────── Trang phục (hàng 12–18 + chân) ─────────────────────────────
// C màu chính · D tối · W trắng · J vest · T cà vạt · S da (tay) · V túi

export interface OutfitDef {
  id: string;
  label: string;
  torso: Layer;
  /** Váy liền tự vẽ phần chân. */
  legs?: Layer;
  /** Nữ mặc bộ này có thể đi chân váy. */
  skirtable?: boolean;
}

export const OUTFITS: OutfitDef[] = [
  {
    id: "tshirt",
    label: "Áo thun",
    torso: [
      "...CCCCSSCCCC...",
      "..CCCCCCCCCCCC..",
      ".CCCCCCCCCCCCCC.",
      ".CCCCCCCCCCCCCC.",
      ".SDCCCCCCCCCCDS.",
      ".S.DCCCCCCCCD.S.",
      "...CCCCCCCCCC...",
    ],
  },
  {
    id: "shirt",
    label: "Sơ mi + cà vạt",
    skirtable: true,
    torso: [
      "...WWWWWWWWWW...",
      "..CCCCWTTWCCCC..",
      ".CCCCCCTTCCCCCC.",
      ".CCCCCCTTCCCCCC.",
      ".SDCCCCTTCCCCDS.",
      ".S.DCCCTTCCCD.S.",
      "...CCCCCCCCCC...",
    ],
  },
  {
    id: "suit",
    label: "Vest",
    torso: [
      "...JJJWWWWJJJ...",
      "..JJJJWTTWJJJJ..",
      ".JJJJJWTTWJJJJJ.",
      ".JJJJJJTTJJJJJJ.",
      ".SJJJJJTTJJJJJS.",
      ".S.JJJJJJJJJJ.S.",
      "...JJJJJJJJJJ...",
    ],
  },
  {
    id: "blazer",
    label: "Blazer",
    skirtable: true,
    torso: [
      "...CCCWWWWCCC...",
      "..CCCCWWWWCCCC..",
      ".CCCCCWWWWCCCCC.",
      ".CCCCCCWWCCCCCC.",
      ".SDCCCCCCCCCCDS.",
      ".S.DCCCCCCCCD.S.",
      "...CCCCCCCCCC...",
    ],
  },
  {
    id: "hoodie",
    label: "Hoodie",
    torso: [
      "..CCCCCSSCCCCC..",
      "..CCCCCWWCCCCC..",
      ".CCCCCCWWCCCCCC.",
      ".CCCCCCCCCCCCCC.",
      ".SCCCDDDDDDCCCS.",
      ".S.CCDDDDDDCC.S.",
      "...CCCCCCCCCC...",
    ],
  },
  {
    id: "lab_coat",
    label: "Blouse phòng lab",
    torso: [
      "...WWWCSSCWWW...",
      "..WWWWCCCCWWWW..",
      ".WWWWWCCCCWWWWW.",
      ".WWWWWCCCCWWWWW.",
      ".SWWVWCCCCWVWWS.",
      ".S.WWWCCCCWWW.S.",
      "...WWWWWWWWWW...",
      "...WWPPPPPPWW...",
    ],
  },
  {
    id: "polo",
    label: "Polo",
    torso: [
      "...CCDDSSDDCC...",
      "..CCCCCWCCCCCC..",
      ".CCCCCCWCCCCCCC.",
      ".CCCCCCCCCCCCCC.",
      ".SDCCCCCCCCCCDS.",
      ".S.DCCCCCCCCD.S.",
      "...CCCCCCCCCC...",
    ],
  },
  {
    id: "cardigan",
    label: "Cardigan",
    skirtable: true,
    torso: [
      "...CCCWSSWCCC...",
      "..CCCCWWWWCCCC..",
      ".CCCCCWWWWCCCCC.",
      ".CCCCCWWWWCCCCC.",
      ".SDCCCWWWWCCCDS.",
      ".S.DCCWWWWCCD.S.",
      "...CCCWWWWCCC...",
    ],
  },
  {
    id: "dress",
    label: "Váy liền",
    torso: [
      "...CCCCSSCCCC...",
      "..CCCCCCCCCCCC..",
      ".SCCCCCCCCCCCCS.",
      ".SCCCCCCCCCCCCS.",
      ".S.DCCCCCCCCD.S.",
      ".S.CCCCCCCCCC.S.",
      "...CCCCCCCCCC...",
    ],
    legs: ["..CCCCCCCCCCCC..", "..DCCCCCCCCCCD..", "....SS....SS....", "....SS....SS....", "...BBB....BBB..."],
  },
];

const PANTS: Layer = ["...PPPPPPPPPP...", "...PPPP..PPPP...", "...PPPP..PPPP...", "...PPPP..PPPP...", "..BBBBB..BBBBB.."];
const SKIRT: Layer = ["...KKKKKKKKKK...", "..KKKKKKKKKKKK..", "....SS....SS....", "....SS....SS....", "...BBB....BBB..."];

// ───────────────────────────── Kiểu tóc ─────────────────────────────

export interface HairDef {
  id: string;
  label: string;
  gender: "male" | "female" | "any";
  layer: Layer;
}

export const HAIRS: HairDef[] = [
  {
    id: "short",
    label: "Ngắn",
    gender: "male",
    layer: at(2, ["....HHHHHHHH....", "...HHHHHHHHHH...", "..HHHHHHHHHHHH..", "..HHHHHHHHHHHH..", "..HH........HH.."]),
  },
  {
    id: "side_part",
    label: "Rẽ ngôi",
    gender: "any",
    layer: at(2, ["....HHHHHHHH....", "...HHHHHHHHHH...", "..HHHHHHHHHHHH..", "..HHHHHHH...HH..", "..H..........H.."]),
  },
  {
    id: "spiky",
    label: "Dựng",
    gender: "male",
    layer: [
      "...H..H..H..H...",
      "...HH.HH.HH.HH..",
      "..HHHHHHHHHHHH..",
      "..HHHHHHHHHHHH..",
      "..HHHHHHHHHHHH..",
      "..HH.HH.HH.HHH..",
    ],
  },
  {
    id: "buzz",
    label: "Cắt sát",
    gender: "male",
    layer: at(2, ["....HHHHHHHH....", "...HHHHHHHHHH...", "..H.HHHHHHHH.H.."]),
  },
  { id: "bald", label: "Hói", gender: "male", layer: at(5, ["..H..........H..", "..H..........H.."]) },
  {
    id: "curly",
    label: "Xoăn",
    gender: "any",
    layer: [
      "....HH.HH.HH....",
      "..HHHHHHHHHHHH..",
      ".HHHHHHHHHHHHHH.",
      ".HHHHHHHHHHHHHH.",
      ".HHHHHHHHHHHHHH.",
      ".HHHHHHHHHHHHHH.",
      ".HH..........HH.",
      ".HH..........HH.",
      "..H..........H..",
    ],
  },
  {
    id: "mohawk",
    label: "Mohawk",
    gender: "male",
    layer: [".......HH.......", "......HHHH......", "......HHHH......", "....HHHHHHHH....", "..H.HHHHHHHH.H.."],
  },
  {
    id: "long",
    label: "Dài thẳng",
    gender: "female",
    layer: at(2, [
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHH..",
      "..HHHHHHHHHHHH..",
      ".HHH........HHH.",
      ".HH..........HH.",
      ".HH..........HH.",
      ".HH..........HH.",
      ".HHH........HHH.",
      ".HHHH......HHHH.",
      ".HHH........HHH.",
      ".HH..........HH.",
      ".HH..........HH.",
    ]),
  },
  {
    id: "wavy",
    label: "Dài gợn sóng",
    gender: "female",
    layer: at(2, [
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHH..",
      ".HHHHH....HHHHH.",
      ".HHH........HHH.",
      "HHH..........HHH",
      ".HH..........HH.",
      "HHH..........HHH",
      ".HHH........HHH.",
      "HHHH........HHHH",
      ".HH..........HH.",
      "HH............HH",
    ]),
  },
  {
    id: "ponytail",
    label: "Đuôi ngựa",
    gender: "female",
    layer: at(2, [
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHHH.",
      "..HHHHHHHHHHHHHH",
      "..H..........HHH",
      ".............HH.",
      "..............HH",
      "..............H.",
    ]),
  },
  {
    id: "bun",
    label: "Búi",
    gender: "female",
    layer: [
      "......HHHH......",
      ".....HHHHHH.....",
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHH..",
      "..HHH......HHH..",
      "..H..........H..",
    ],
  },
  {
    id: "bob",
    label: "Bob",
    gender: "female",
    layer: at(2, [
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHH..",
      "..HHHHHHHHHHHH..",
      ".HHH........HHH.",
      ".HH..........HH.",
      ".HH..........HH.",
      ".HHH........HHH.",
      "..HH........HH..",
    ]),
  },
  {
    id: "twin_tails",
    label: "Hai bím",
    gender: "female",
    layer: at(2, [
      "....HHHHHHHH....",
      "...HHHHHHHHHH...",
      "..HHHHHHHHHHHH..",
      "..HHHHHHHHHHHH..",
      "HHH..........HHH",
      "HH............HH",
      "HH............HH",
      "HH............HH",
      ".H............H.",
      ".H............H.",
    ]),
  },
];

// ───────────────────────────── Kính + phụ kiện ─────────────────────────────

export const EYEWEAR: { id: string; label: string; layer: Layer }[] = [
  { id: "none", label: "Không", layer: [] },
  // Gọng tròn rộng hơn mắt (mắt ở cột 4–5 và 10–11, hàng 6–7) → còn 1px da hai bên, nhìn rõ mắt.
  {
    id: "glasses",
    label: "Kính cận",
    layer: at(5, ["...GGGG..GGGG...", "..G....GG....G..", "..G....GG....G..", "...GGGG..GGGG..."]),
  },
  { id: "sunglasses", label: "Kính râm", layer: at(6, ["..GGGGGGGGGGGG..", "...GGGG..GGGG..."]) },
];

/** Q màu phụ kiện (mũ…) · Y vàng · K xám · L gỗ · F hoa · U cốc */
export const ACCESSORIES: { id: string; label: string; layer: Layer }[] = [
  { id: "none", label: "Không", layer: [] },
  {
    id: "headphones",
    label: "Tai nghe",
    layer: at(1, ["....KKKKKKKK....", "...K........K...", "..K..........K..", "..K..........K..", ".KK..........KK.", ".KK..........KK.", ".KK..........KK."]),
  },
  {
    id: "headset",
    label: "Headset + mic",
    layer: at(1, [
      "....KKKKKKKK....",
      "...K........K...",
      "..K..........K..",
      "..K..........K..",
      ".KK..........KK.",
      ".KK..........KK.",
      ".KKK.........KK.",
      "...KK...........",
      "....KK..........",
    ]),
  },
  { id: "crown", label: "Vương miện", layer: ["...Y..Y..Y..Y...", "...YYYYYYYYYY..."] },
  {
    id: "cap",
    label: "Mũ lưỡi trai",
    layer: at(1, ["....QQQQQQQQ....", "...QQQQQQQQQQ...", "..QQQQQQQQQQQQ..", "..QQQQQQQQQQQQQQ"]),
  },
  {
    id: "beanie",
    label: "Mũ len",
    layer: at(0, [".......QQ.......", "....QQQQQQQQ....", "...QQQQQQQQQQ...", "..QQQQQQQQQQQQ..", "..qqqqqqqqqqqq.."]),
  },
  { id: "earrings", label: "Khuyên tai", layer: at(8, [".Y............Y.", ".Y............Y."]) },
  { id: "bowtie", label: "Nơ", layer: at(12, ["......TTTT......", "......T..T......"]) },
  { id: "badge", label: "Thẻ tên", layer: at(14, ["..........WW....", "..........QQ...."]) },
  { id: "clipboard", label: "Clipboard", layer: at(14, ["...........LLLL.", "...........LWWL.", "...........LWWL.", "...........LLLL."]) },
  { id: "mug", label: "Cốc cà phê", layer: at(15, ["............UUU.", "............UUUU", "............UUU."]) },
  { id: "flower", label: "Hoa cài tóc", layer: at(2, ["...........FF...", "..........FYYF..", "...........FF..."]) },
];

// ───────────────────────────── Mẫu nhân vật có sẵn ─────────────────────────────

export interface CharacterPreset {
  id: string;
  label: string;
  look: Appearance;
}

const look = (
  gender: string,
  skin: string,
  hair_style: string,
  hair_color: string,
  outfit: string,
  outfit_color: string,
  eyewear: string,
  accessory: string,
): Appearance => ({ gender, skin, hair_style, hair_color, outfit, outfit_color, eyewear, accessory });

export const PRESETS: CharacterPreset[] = [
  { id: "exec_m", label: "Giám đốc lịch lãm", look: look("male", "light", "side_part", "gray", "suit", "#1f2937", "glasses", "none") },
  { id: "exec_f", label: "Nữ CEO quyền lực", look: look("female", "beige", "bob", "black", "blazer", "#7c2d12", "none", "earrings") },
  { id: "boss_bald", label: "Sếp tóc bạc", look: look("male", "tan", "bald", "white", "suit", "#334155", "glasses", "bowtie") },
  { id: "dev_hoodie", label: "Dev hoodie", look: look("male", "olive", "spiky", "black", "hoodie", "#2563eb", "none", "headphones") },
  { id: "dev_pink", label: "Nữ dev tóc hồng", look: look("female", "porcelain", "ponytail", "pink", "hoodie", "#6d28d9", "none", "headphones") },
  { id: "hacker", label: "Hacker bảo mật", look: look("male", "brown", "mohawk", "green", "hoodie", "#111827", "sunglasses", "none") },
  { id: "scientist", label: "Nhà khoa học dữ liệu", look: look("female", "dark", "curly", "black", "lab_coat", "#0e7490", "glasses", "none") },
  { id: "sre", label: "Kỹ sư SRE trực ca", look: look("male", "deep", "buzz", "black", "tshirt", "#0f766e", "none", "headset") },
  { id: "designer", label: "Designer cá tính", look: look("female", "light", "wavy", "purple", "dress", "#db2777", "none", "flower") },
  { id: "tester", label: "Tester tỉ mỉ", look: look("female", "tan", "bun", "dark_brown", "polo", "#65a30d", "glasses", "clipboard") },
  { id: "pm", label: "PM năng động", look: look("female", "olive", "twin_tails", "auburn", "cardigan", "#0284c7", "none", "badge") },
  { id: "analyst", label: "Analyst cà phê", look: look("male", "beige", "curly", "brown", "shirt", "#475569", "glasses", "mug") },
  { id: "marketer", label: "Marketer UA", look: look("female", "brown", "long", "blonde", "blazer", "#be185d", "sunglasses", "none") },
  { id: "gamer", label: "Game designer", look: look("male", "light", "short", "blue", "tshirt", "#ea580c", "none", "cap") },
];

// ───────────────────────────── Sinh sprite ─────────────────────────────

/** FNV-1a + avalanche (murmur3 fmix32) → id gần giống nhau vẫn cho thuộc tính phân bố đều. */
function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b);
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35);
  h ^= h >>> 16;
  return h >>> 0;
}

const pick = <T,>(arr: readonly T[], n: number): T => arr[n % arr.length]!;

export function shade(hex: string, f: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1]!, 16);
  const c = [n >> 16, (n >> 8) & 255, n & 255].map((v) => Math.max(0, Math.min(255, Math.round(v * f))));
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

const isHex = (s: string | undefined): s is string => !!s && /^#[0-9a-f]{6}$/i.test(s);

/** Ngoại hình đầy đủ sau khi điền các trường "tự động". */
export interface ResolvedLook {
  gender: "male" | "female";
  skin: string;
  hairStyle: string;
  hairColor: string;
  outfit: string;
  outfitColor: string;
  eyewear: string;
  accessory: string;
  skirt: boolean;
}

export interface SpriteInput {
  id: string;
  color: string;
  level: RoleLevel;
  icon?: string;
  appearance?: Partial<Appearance> | null;
}

export function resolveLook({ id, color, level, icon, appearance: a }: SpriteInput): ResolvedLook {
  const h = hash(id);
  const r = (salt: number) => hash(`${id}#${salt}`);
  const gender: "male" | "female" = a?.gender === "male" || a?.gender === "female" ? a.gender : h & 1 ? "female" : "male";

  const hairPool = HAIRS.filter((x) => x.gender === "any" || x.gender === gender).filter((x) => x.id !== "bald" || level === "c_level");
  const hairStyle = HAIRS.some((x) => x.id === a?.hair_style) ? a!.hair_style! : pick(hairPool, r(1)).id;

  const naturals = HAIR_COLORS.filter((c) => c.natural && (c.id !== "gray" || level === "c_level" || level === "head"));
  const hairColor = isHex(a?.hair_color)
    ? a!.hair_color!
    : (HAIR_COLORS.find((c) => c.id === a?.hair_color) ?? (r(2) % 9 === 0 ? pick(HAIR_COLORS.filter((c) => !c.natural), r(3)) : pick(naturals, r(3)))).color;

  const skin = isHex(a?.skin) ? a!.skin! : (SKINS.find((s) => s.id === a?.skin) ?? pick(SKINS, r(4))).color;

  const byLevel: Record<RoleLevel, [string[], string[]]> = {
    c_level: [["suit"], ["blazer", "suit"]],
    head: [["suit", "blazer"], ["blazer", "cardigan"]],
    manager: [["shirt", "polo", "blazer"], ["blazer", "cardigan", "shirt", "dress"]],
    lead: [["polo", "cardigan", "shirt"], ["polo", "cardigan", "hoodie"]],
    engineer: [["hoodie", "tshirt", "polo"], ["hoodie", "tshirt", "cardigan"]],
    specialist: [["tshirt", "polo", "lab_coat"], ["tshirt", "polo", "dress", "lab_coat"]],
  };
  const outfit = OUTFITS.some((o) => o.id === a?.outfit)
    ? a!.outfit!
    : pick(byLevel[level][gender === "male" ? 0 : 1], r(5));
  const def = OUTFITS.find((o) => o.id === outfit)!;

  const eyewear = EYEWEAR.some((e) => e.id === a?.eyewear)
    ? a!.eyewear!
    : level === "lead" || r(6) % 5 === 0
      ? "glasses"
      : "none";

  let accessory = ACCESSORIES.some((x) => x.id === a?.accessory) ? a!.accessory! : "none";
  if (!a?.accessory) {
    if (icon === "crown") accessory = "crown";
    else if (level === "engineer" && r(7) % 2 === 0) accessory = "headphones";
    else if (level === "specialist" && r(7) % 2 === 0) accessory = "clipboard";
    else if (gender === "female" && r(7) % 4 === 0) accessory = "earrings";
  }

  return {
    gender,
    skin,
    hairStyle,
    hairColor,
    outfit,
    outfitColor: isHex(a?.outfit_color) ? a!.outfit_color! : color,
    eyewear,
    accessory,
    skirt: gender === "female" && !!def.skirtable && r(8) % 2 === 0,
  };
}

type Grid = (string | null)[][];
const newGrid = (): Grid => Array.from({ length: SPRITE_H }, () => Array<string | null>(SPRITE_W).fill(null));

function paint(grid: Grid, layer: Layer, pal: Record<string, string>, dy = 0) {
  layer.forEach((row, y) => {
    const gy = y + dy;
    if (gy < 0 || gy >= SPRITE_H) return;
    for (let x = 0; x < SPRITE_W && x < row.length; x++) {
      const ch = row[x]!;
      if (ch === ".") continue;
      const fill = pal[ch];
      if (fill) grid[gy]![x] = fill;
    }
  });
}

/** Ghép các run pixel cùng màu trên 1 hàng thành 1 rect → ít node SVG. */
function toRects(grid: Grid): Rect[] {
  const out: Rect[] = [];
  grid.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      const fill = row[x];
      if (!fill) {
        x++;
        continue;
      }
      let w = 1;
      while (x + w < row.length && row[x + w] === fill) w++;
      out.push({ x, y, w, h: 1, fill });
      x += w;
    }
  });
  return out;
}

/** Tô sáng mép trên của tóc → có khối, đỡ phẳng. */
function highlightHair(grid: Grid, hair: string) {
  const light = shade(hair, hair === "#1c1917" ? 2.2 : 1.25);
  for (let x = 0; x < SPRITE_W; x++) {
    for (let y = 0; y < SPRITE_H; y++) {
      if (grid[y]![x] === hair) {
        if (y === 0 || grid[y - 1]![x] !== hair) grid[y]![x] = light;
        break;
      }
    }
  }
}

const cache = new Map<string, Sprite>();

export function makeSprite(input: SpriteInput): Sprite {
  const key = JSON.stringify([input.id, input.color, input.level, input.icon ?? "", input.appearance ?? null]);
  const hit = cache.get(key);
  if (hit) return hit;

  const lk = resolveLook(input);
  const def = OUTFITS.find((o) => o.id === lk.outfit) ?? OUTFITS[0]!;
  const c = lk.outfitColor;
  const jacket = lk.outfit === "suit" ? (isHex(input.appearance?.outfit_color) ? c : "#262b3d") : shade(c, 0.6);
  const pal: Record<string, string> = {
    S: lk.skin,
    E: "#1f1b2e",
    M: lk.gender === "female" ? "#d9536f" : "#9f3a3a",
    R: shade(lk.skin, 0.88),
    H: lk.hairColor,
    q: shade(lk.hairColor, 0.7),
    C: c,
    D: shade(c, 0.72),
    W: "#f1f5f9",
    J: jacket,
    T: lk.outfit === "suit" ? (isHex(input.appearance?.outfit_color) ? input.color : input.color) : shade(c, 0.5),
    V: "#cbd5e1",
    P: lk.outfit === "suit" ? shade(jacket, 0.9) : "#2d3047",
    K: lk.outfit === "suit" ? jacket : shade(c, 0.55),
    B: "#14141f",
    G: "#111827",
    Y: "#facc15",
    L: "#a16207",
    F: "#f472b6",
    U: "#f8fafc",
    Q: lk.outfit === "suit" ? input.color : shade(c, 0.85),
  };
  // Mũ len dùng màu riêng cho lớp gập
  pal.q = shade(pal.Q!, 0.75);

  const grid = newGrid();
  paint(grid, BODY, pal);
  paint(grid, lk.gender === "female" ? FACE_FEMALE : FACE_MALE, pal);
  paint(grid, def.torso, pal, 12);
  paint(grid, def.legs ?? (lk.skirt ? SKIRT : PANTS), pal, 12 + def.torso.length);
  // Mũ che phần tóc trên; tóc vẽ trước để phần dài hai bên vẫn lộ ra.
  const hairLayer = HAIRS.find((x) => x.id === lk.hairStyle)?.layer ?? [];
  paint(grid, hairLayer, pal);
  highlightHair(grid, lk.hairColor);
  paint(grid, EYEWEAR.find((e) => e.id === lk.eyewear)?.layer ?? [], pal);
  paint(grid, ACCESSORIES.find((x) => x.id === lk.accessory)?.layer ?? [], pal);

  const overlay = (layer: Layer) => {
    const g = newGrid();
    paint(g, layer, pal);
    return toRects(g);
  };

  const sprite: Sprite = {
    body: toRects(grid),
    mouthOpen: overlay(MOUTH_OPEN),
    blink: lk.eyewear === "sunglasses" ? [] : overlay(BLINK),
  };
  cache.set(key, sprite);
  return sprite;
}

/** Kiểm tra template (dev): mọi hàng đúng 16 ký tự. */
export function validateTemplates(): string[] {
  const errs: string[] = [];
  const check = (name: string, layer: Layer) =>
    layer.forEach((row, i) => {
      if (row.length !== SPRITE_W) errs.push(`${name}[${i}] dài ${row.length}`);
    });
  check("BODY", BODY);
  check("FACE_MALE", FACE_MALE);
  check("FACE_FEMALE", FACE_FEMALE);
  check("PANTS", PANTS);
  check("SKIRT", SKIRT);
  OUTFITS.forEach((o) => {
    check(`outfit.${o.id}`, o.torso);
    if (o.legs) check(`outfit.${o.id}.legs`, o.legs);
  });
  HAIRS.forEach((x) => check(`hair.${x.id}`, x.layer));
  EYEWEAR.forEach((x) => check(`eyewear.${x.id}`, x.layer));
  ACCESSORIES.forEach((x) => check(`acc.${x.id}`, x.layer));
  return errs;
}
