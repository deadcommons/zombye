#!/usr/bin/env python3
"""Writes svg/<id>.svg for every item in items.tsv. Tokens: $C main, $D dark, $L light, $K hole."""
import os
HERE = os.path.dirname(os.path.abspath(__file__))
K = '#17181a'

def tones(hexc):
    r, g, b = [int(hexc[i:i+2], 16) for i in (1, 3, 5)]
    if max(r, g, b) < 0x50:               # very dark: scale up so it reads on the dark tile
        s = 0x78 / max(r, g, b, 1)
        r, g, b = [min(255, int(v * s)) for v in (r, g, b)]
    h = lambda r, g, b: '#%02x%02x%02x' % (r, g, b)
    C = h(r, g, b)
    D = h(int(r*.6), int(g*.6), int(b*.6))
    L = h(*[int(v + (255 - v) * .45) for v in (r, g, b)])
    return C, D, L

def rot(s, a=-45): return f'<g transform="rotate({a} 24 24)">{s}</g>'
def tin(extra, top='$L'):
    return ('<rect x="12" y="11" width="24" height="29" rx="2.5" fill="$C"/>'
            '<rect x="12" y="18" width="24" height="15" fill="$D"/>' + extra +
            f'<ellipse cx="24" cy="11" rx="12" ry="3.5" fill="{top}"/><rect x="12" y="37" width="24" height="3" rx="1.5" fill="$D"/>')
def bottle(body, cap='$D'):
    return body
def can_stroke(d, col='$C', w=3): return f'<path d="{d}" stroke="{col}" stroke-width="{w}" fill="none" stroke-linecap="round" stroke-linejoin="round"/>'

I = {}
# ---------- Food
I['tinned_beans'] = tin('<ellipse cx="19" cy="25" rx="3" ry="2" fill="$L"/><ellipse cx="25" cy="23" rx="3" ry="2" fill="$L"/><ellipse cx="29" cy="28" rx="3" ry="2" fill="$L"/><ellipse cx="21" cy="29" rx="3" ry="2" fill="$L"/>')
I['tinned_soup'] = tin('<rect x="12" y="18" width="24" height="3" fill="$L"/><rect x="12" y="30" width="24" height="3" fill="$L"/><circle cx="24" cy="25.5" r="4" fill="$C"/><path d="M21 25h6" stroke="$L" stroke-width="1.6"/>')
I['tinned_fruit'] = tin('<circle cx="22" cy="26" r="5" fill="$L"/><path d="M24 20c1-2 3-3 5-3-1 2-3 3-5 3z" fill="$C"/>')
I['cereal'] = ('<path d="M9 9h22v32H9z" fill="$C"/><path d="M31 9l8-4v32l-8 4z" fill="$D"/><path d="M9 9l8-4h22l-8 4z" fill="$L"/>'
               '<ellipse cx="20" cy="29" rx="9" ry="5.5" fill="$L"/><path d="M12 28h16c0 5-4 8-8 8s-8-3-8-8z" fill="$D"/>'
               '<circle cx="16" cy="25" r="2.2" fill="$C"/><circle cx="21" cy="23" r="2.2" fill="$C"/><circle cx="25" cy="26" r="2.2" fill="$C"/>')
I['peanut_butter'] = ('<rect x="9" y="8" width="30" height="8" rx="2.5" fill="$D"/><path d="M9 18c0-3 3-3 3-3h24s3 0 3 3v20a4 4 0 0 1-4 4H13a4 4 0 0 1-4-4z" fill="$C"/>'
                      '<rect x="9" y="22" width="30" height="14" fill="$L"/><ellipse cx="24" cy="29" rx="7" ry="4" fill="$D"/><rect x="11" y="16" width="4" height="22" rx="2" fill="$L"/>')
I['chocolate_bar'] = rot('<rect x="6" y="14" width="36" height="20" rx="2.5" fill="$D"/><rect x="8" y="16" width="32" height="16" rx="1.5" fill="$C"/>'
                         '<path d="M16 16v16M24 16v16M32 16v16M8 24h32" stroke="$D" stroke-width="1.6"/><path d="M6 16h13v16H6z" fill="$L"/>'
                         '<rect x="6" y="14" width="13" height="20" rx="2" fill="$L"/><path d="M9 18h7M9 22h7" stroke="$D" stroke-width="1.6"/>')
I['crisps'] = ('<path d="M10 7h28l-2 6 2 6-2 6 2 8-2 6H12l-2-6 2-8-2-6 2-6z" fill="$C"/><path d="M10 7h28v4H10zM10 38h28l-1 4H11z" fill="$D"/>'
               '<path d="M10 7l2 3 2-3 2 3 2-3 2 3 2-3 2 3 2-3 2 3 2-3 2 3 2-3v4H10z" fill="$L"/>'
               '<ellipse cx="24" cy="25" rx="9" ry="7" fill="$L"/><path d="M18 24c3-3 5 3 8 0s3 2 5 0" stroke="$D" stroke-width="1.6" fill="none"/>')
I['bread'] = ('<path d="M5 25c0-9 7-13 19-13s19 4 19 13v11a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2z" fill="$C"/><path d="M5 25c0-9 7-13 19-13s19 4 19 13c-4 2-9 0-19 0S9 27 5 25z" fill="$L"/>'
         '<path d="M14 17l3 6M22 15l3 6M30 16l3 6" stroke="$D" stroke-width="2.2" stroke-linecap="round"/><rect x="5" y="34" width="38" height="4" rx="2" fill="$D"/>')
I['apple'] = ('<path d="M24 14c-8-5-15 1-13 11 2 9 8 15 13 12 5 3 11-3 13-12 2-10-5-16-13-11z" fill="$C"/><path d="M24 14c0-4 2-7 5-8" stroke="$D" stroke-width="2.5" fill="none" stroke-linecap="round"/>'
              '<path d="M28 10c3-3 7-2 8 0-3 3-6 3-8 0z" fill="$L"/><ellipse cx="17" cy="22" rx="2.5" ry="4.5" fill="$L"/>')
I['cheese'] = ('<path d="M7 25h27v15H7z" fill="$C"/><path d="M7 25l8-10h27l-8 10z" fill="$L"/><path d="M34 25l8-10v15l-8 10z" fill="$D"/>'
               '<circle cx="15" cy="32" r="2.6" fill="$D"/><circle cx="26" cy="35" r="2" fill="$D"/><circle cx="26" cy="29" r="1.5" fill="$D"/><ellipse cx="29" cy="20" rx="3" ry="1.4" fill="$C"/>')
# ---------- Drink
I['water_bottle'] = ('<path d="M20 6h8v7l5 6v20a3 3 0 0 1-3 3H18a3 3 0 0 1-3-3V19l5-6z" fill="$L"/><path d="M15 24h18v15a3 3 0 0 1-3 3H18a3 3 0 0 1-3-3z" fill="$C"/>'
                     '<rect x="19" y="4" width="10" height="5" rx="1.5" fill="$D"/><rect x="15" y="29" width="18" height="6" fill="$D"/><path d="M15 24c3-2 5 2 9 0s6 2 9 0v2H15z" fill="$L"/>')
I['empty_bottle'] = rot('<path d="M4 20h8l5-3h17a4 4 0 0 1 4 4v6a4 4 0 0 1-4 4H17l-5-3H4z" fill="$C"/><path d="M17 17h17a4 4 0 0 1 4 4v1H17z" fill="$L"/>'
                        '<rect x="0" y="19" width="6" height="10" rx="1.5" fill="$D"/><rect x="21" y="25" width="14" height="2" rx="1" fill="$L"/><path d="M17 17v14" stroke="$D" stroke-width="1.6"/>', -35)
I['soda_can'] = ('<rect x="14" y="9" width="20" height="31" rx="3" fill="$C"/><rect x="14" y="17" width="20" height="14" fill="$D"/><ellipse cx="24" cy="9" rx="10" ry="3" fill="$L"/>'
                 '<ellipse cx="24" cy="9" rx="4.5" ry="1.4" fill="$D"/><path d="M14 27c4-5 8 1 12-3s6 0 8-2v5H14z" fill="$L"/><rect x="14" y="37" width="20" height="3" rx="1.5" fill="$D"/>')
I['juice_carton'] = ('<path d="M12 16h18v26H12z" fill="$C"/><path d="M30 16l8-4v26l-8 4z" fill="$D"/><path d="M12 16l6-9h12l8 5-8 4z" fill="$L"/><path d="M18 7l12 9" stroke="$D" stroke-width="1.5"/>'
                    '<circle cx="21" cy="29" r="6" fill="$L"/><path d="M21 23c0-2 1-3 3-3-1 2-1 3-3 3z" fill="$D"/>')
I['milk'] = ('<path d="M19 5h10v5l4 5v22a4 4 0 0 1-4 4H19a4 4 0 0 1-4-4V15l4-5z" fill="$C"/><rect x="18" y="3" width="12" height="5" rx="1.5" fill="$D"/>'
             '<path d="M33 20h4a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3h-4" stroke="$C" stroke-width="3.5" fill="none"/><rect x="15" y="24" width="18" height="10" fill="$L"/><circle cx="24" cy="29" r="3.2" fill="$D"/>')
# ---------- Medical
I['plasters'] = ('<rect x="5" y="13" width="38" height="24" rx="3" fill="$C"/><rect x="5" y="13" width="38" height="5" rx="2" fill="$L"/><rect x="5" y="33" width="38" height="4" rx="2" fill="$D"/>'
                 + rot('<rect x="10" y="19" width="28" height="10" rx="5" fill="$L"/><rect x="19" y="19" width="10" height="10" fill="$D"/><circle cx="22" cy="22" r=".9" fill="$L"/><circle cx="26" cy="22" r=".9" fill="$L"/><circle cx="22" cy="26" r=".9" fill="$L"/><circle cx="26" cy="26" r=".9" fill="$L"/>', -20))
I['gauze'] = ('<ellipse cx="18" cy="26" rx="10" ry="12" fill="$C"/><path d="M18 14h14c5 0 8 5 8 12s-3 12-8 12H18c5 0 8-5 8-12s-3-12-8-12z" fill="$L"/><ellipse cx="18" cy="26" rx="3.5" ry="4.5" fill="$K"/>'
              '<path d="M30 38h12v5H26z" fill="$L"/><path d="M33 40h7" stroke="$D" stroke-width="1.5"/>')
I['rag'] = ('<path d="M7 13l8-3 8 3 8-3 10 3-3 8 3 8-3 8 3 6-11-2-8 3-8-3-10 2 3-7-3-8 3-8z" fill="$C"/><path d="M14 18l20 0M13 25l22 0M14 32l20 0" stroke="$D" stroke-width="1.6" stroke-dasharray="4 2"/>'
         '<path d="M7 13l8-3 8 3 8-3 10 3-3 5H8z" fill="$L"/>')
I['clean_rag'] = ('<rect x="8" y="10" width="32" height="30" rx="3" fill="$C"/><rect x="8" y="10" width="32" height="8" rx="3" fill="$L"/><rect x="8" y="35" width="32" height="5" rx="2" fill="$D"/>'
                  '<path d="M8 22h32" stroke="$D" stroke-width="1.5"/><path d="M28 10h12v12z" fill="$D"/><path d="M28 10l12 12" stroke="$L" stroke-width="1.5"/>')
I['antiseptic'] = ('<rect x="14" y="19" width="20" height="23" rx="4" fill="$C"/><rect x="19" y="13" width="10" height="7" fill="$D"/><ellipse cx="24" cy="9" rx="4.5" ry="6" fill="$D"/>'
                   '<rect x="14" y="26" width="20" height="11" fill="$L"/><path d="M24 28c2 3 3 4 3 6a3 3 0 0 1-6 0c0-2 1-3 3-6z" fill="$D"/>')
I['painkillers'] = ('<rect x="13" y="14" width="22" height="28" rx="4" fill="$C"/><rect x="11" y="6" width="26" height="9" rx="3" fill="$L"/><path d="M15 8v5M19 8v5M23 8v5M27 8v5M31 8v5" stroke="$D" stroke-width="1.2"/>'
                    '<rect x="13" y="22" width="22" height="14" fill="$L"/><path d="M24 24v10M19 29h10" stroke="$D" stroke-width="3.5"/>')
I['antibiotics'] = ('<rect x="9" y="6" width="30" height="36" rx="4" fill="$C"/><rect x="9" y="6" width="30" height="5" rx="2" fill="$L"/>'
                    + ''.join(f'<ellipse cx="{x}" cy="{y}" rx="4" ry="5" fill="$D"/><ellipse cx="{x}" cy="{y-1}" rx="3.4" ry="4.2" fill="$L"/>' for x in (17, 31) for y in (17, 26, 35)))
# ---------- Tools
I['tin_opener'] = rot('<rect x="4" y="21" width="24" height="7" rx="3" fill="$C"/><rect x="8" y="22.5" width="14" height="2" rx="1" fill="$L"/>'
                      '<path d="M26 16h12l4 6-3 12H26z" fill="$L"/><circle cx="35" cy="26" r="4.5" fill="$D"/><circle cx="35" cy="26" r="1.6" fill="$K"/><rect x="30" y="10" width="4" height="8" rx="1.5" fill="$C"/><circle cx="32" cy="9" r="4" fill="$C"/>', -20)
I['screwdriver'] = rot('<rect x="2" y="23" width="22" height="2.6" fill="$L"/><path d="M0 22.5l3 1.3-3 1.4z" fill="$L"/><rect x="24" y="19" width="19" height="10" rx="4.5" fill="$C"/><rect x="22" y="21" width="4" height="6" fill="$D"/><path d="M30 20v8M35 20v8M40 20v8" stroke="$D" stroke-width="1.6"/>')
I['scissors'] = ('<path d="M24 26L17 4l4-1 7 22z" fill="$L"/><path d="M24 26L31 4l-4-1-7 22z" fill="$D"/><path d="M24 24L17 36M24 24L31 36" stroke="$C" stroke-width="3.5" stroke-linecap="round"/>'
                 '<circle cx="14" cy="38" r="5.5" fill="none" stroke="$C" stroke-width="3.5"/><circle cx="34" cy="38" r="5.5" fill="none" stroke="$C" stroke-width="3.5"/><circle cx="24" cy="25" r="2.3" fill="$D"/>')
I['saw'] = rot('<path d="M3 16h33v11l-3 .5-1.5 2-2-2-2 2-2-2-2 2-2-2-2 2-2-2-2 2-2-2-2 2-2-2-2 2-1.5-2H3z" fill="$L"/>'
               '<path d="M3 24h30" stroke="$D" stroke-width="1.5"/><path d="M34 12h10v22H34z" fill="$C"/><rect x="38" y="16" width="3.5" height="9" rx="1.7" fill="$K"/><rect x="33" y="12" width="3" height="22" fill="$D"/>')
I['lighter'] = ('<rect x="14" y="19" width="20" height="23" rx="3" fill="$C"/><rect x="14" y="19" width="20" height="9" rx="2" fill="$L"/><rect x="26" y="14" width="8" height="6" rx="1" fill="$D"/>'
                '<rect x="17" y="14" width="6" height="6" fill="$D"/><path d="M20 14c-4-4 1-6 0-10 4 2 6 6 3 10z" fill="$L"/><rect x="17" y="32" width="14" height="6" rx="1.5" fill="$D"/>')
I['matches'] = (''.join(f'<rect x="{x}" y="6" width="3" height="20" fill="$L"/><circle cx="{x+1.5}" cy="7" r="3.2" fill="$D"/>' for x in (13, 22, 31)) +
                '<rect x="8" y="22" width="32" height="18" rx="2" fill="$C"/><rect x="8" y="22" width="32" height="5" fill="$L"/><rect x="8" y="33" width="32" height="7" rx="2" fill="$D"/><path d="M13 30h22" stroke="$L" stroke-width="1.5"/>')
I['duct_tape'] = ('<circle cx="24" cy="24" r="16" fill="$C"/><circle cx="24" cy="24" r="16" fill="none" stroke="$D" stroke-width="2"/><circle cx="24" cy="24" r="8" fill="$K"/><circle cx="24" cy="24" r="8" fill="none" stroke="$D" stroke-width="2"/>'
                  '<path d="M10 14a16 16 0 0 1 10-7" stroke="$L" stroke-width="3" fill="none" stroke-linecap="round"/><path d="M36 36l8 5-4 3-8-3z" fill="$L"/>')
I['nails'] = (''.join(f'<g transform="rotate({a} {x} 30)"><rect x="{x-1}" y="8" width="2.4" height="24" fill="$L"/><rect x="{x-3.5}" y="6" width="7.4" height="3" rx="1" fill="$L"/></g>' for x, a in ((15, -12), (24, 0), (33, 12))) +
              '<rect x="7" y="26" width="34" height="15" rx="2.5" fill="$C"/><rect x="7" y="26" width="34" height="4" rx="2" fill="$D"/><rect x="7" y="35" width="34" height="6" rx="2" fill="$D"/>')
I['rope'] = ('<ellipse cx="22" cy="27" rx="16" ry="12" fill="none" stroke="$C" stroke-width="5"/><ellipse cx="22" cy="27" rx="9" ry="6" fill="none" stroke="$D" stroke-width="4"/>'
             '<path d="M10 20c4-5 10-7 17-6M34 33c-4 3-9 4-14 4" stroke="$L" stroke-width="2" fill="none" stroke-dasharray="3 3"/><path d="M34 17l5-6 4 1" stroke="$C" stroke-width="4" fill="none" stroke-linecap="round"/>')
I['cable_tie'] = ('<circle cx="21" cy="20" r="12" fill="none" stroke="$C" stroke-width="4.5"/><path d="M12 14a12 12 0 0 1 8-6" stroke="$L" stroke-width="2" fill="none"/>'
                  '<rect x="15" y="30" width="12" height="9" rx="2" fill="$L"/><rect x="19" y="33" width="4" height="3" fill="$K"/><path d="M27 35c6 0 10 2 15 6" stroke="$C" stroke-width="4" fill="none" stroke-linecap="round"/>')
I['roller_blind'] = ('<rect x="5" y="5" width="38" height="7" rx="3" fill="$L"/><rect x="3" y="4" width="4" height="9" rx="1.5" fill="$D"/><rect x="41" y="4" width="4" height="9" rx="1.5" fill="$D"/>'
                     '<rect x="8" y="12" width="32" height="27" fill="$C"/><path d="M8 18h32M8 24h32M8 30h32" stroke="$D" stroke-width="1.2"/><rect x="8" y="12" width="5" height="27" fill="$L"/>'
                     '<rect x="7" y="38" width="34" height="3.5" rx="1.5" fill="$D"/><path d="M24 41.5v3" stroke="$L" stroke-width="2"/><circle cx="24" cy="45" r="1.8" fill="$L"/>')
# ---------- Light
I['flashlight'] = rot('<rect x="14" y="19" width="30" height="10" rx="3" fill="$C"/><path d="M3 13h12l1 22H3z" fill="$D"/><path d="M2 12h4v24H2z" fill="$L"/><path d="M9 15l5 3v12l-5 3z" fill="$L"/><rect x="26" y="21" width="7" height="3.5" rx="1" fill="$L"/><path d="M18 20v8M22 20v8" stroke="$D" stroke-width="1.5"/>')
I['batteries'] = ''.join(f'<rect x="{x}" y="12" width="14" height="30" rx="2.5" fill="$C"/><rect x="{x+4}" y="7" width="6" height="5" rx="1" fill="$L"/><rect x="{x}" y="12" width="14" height="8" rx="2" fill="$L"/><rect x="{x}" y="28" width="14" height="8" fill="$D"/>' for x in (8, 26))
I['candle'] = ('<ellipse cx="24" cy="42" rx="14" ry="3.5" fill="$D"/><rect x="17" y="18" width="14" height="24" rx="2" fill="$C"/><ellipse cx="24" cy="18" rx="7" ry="2.4" fill="$L"/><rect x="17" y="18" width="4" height="24" fill="$L"/>'
               '<path d="M27 20v7a1.8 1.8 0 0 0 3.6 0v-7z" fill="$D"/><path d="M24 17v-3" stroke="$D" stroke-width="1.5"/><path d="M24 14c-4-3-3-7 0-11 3 4 4 8 0 11z" fill="$L"/>')
# ---------- Material
I['towel'] = ('<rect x="7" y="13" width="31" height="22" rx="3" fill="$C"/><ellipse cx="38" cy="24" rx="5" ry="11" fill="$L"/><path d="M38 24m-2.5 0a2.5 5 0 1 0 5 0a2.5 5 0 1 0-5 0" fill="none" stroke="$D" stroke-width="1.6"/>'
                '<rect x="13" y="13" width="5" height="22" fill="$D"/><rect x="24" y="13" width="5" height="22" fill="$D"/><path d="M7 38v5M12 38v4M17 38v5" stroke="$C" stroke-width="2" stroke-linecap="round"/>')
I['thread'] = ('<rect x="12" y="6" width="24" height="5" rx="2" fill="$D"/><rect x="12" y="37" width="24" height="5" rx="2" fill="$D"/><rect x="14" y="11" width="20" height="26" fill="$C"/>'
               '<path d="M14 16l20 -3M14 22l20-3M14 28l20-3M14 34l20-3" stroke="$L" stroke-width="2"/><rect x="14" y="11" width="5" height="26" fill="$L"/><path d="M34 34c4 0 7 2 9 6" stroke="$L" stroke-width="2" fill="none" stroke-linecap="round"/>')
# ---------- Clothing
I['t_shirt'] = ('<path d="M16 8L4 15l5 9 6-3v20h18V21l6 3 5-9-12-7c-2 3-5 4-8 4s-6-1-8-4z" fill="$C"/><path d="M16 8c2 3 5 4 8 4s6-1 8-4" stroke="$D" stroke-width="2.5" fill="none"/>'
                '<path d="M4 15l5 9 4-2-3-9z" fill="$D"/><path d="M44 15l-5 9-4-2 3-9z" fill="$D"/><rect x="15" y="38" width="18" height="2.5" fill="$L"/>')
def long_top(extra, neck):
    return ('<path d="M16 8L7 12 3 35l8 2 3-15v20h20V22l3 15 8-2-4-23-9-4c-2 3-5 4-8 4s-6-1-8-4z" fill="$C"/>' + neck + extra)
I['jumper'] = long_top('<rect x="3" y="33" width="8.5" height="5" rx="1.5" fill="$D"/><rect x="36.5" y="33" width="8.5" height="5" rx="1.5" fill="$D"/><rect x="14" y="37" width="20" height="5" rx="1.5" fill="$D"/>'
                       '<path d="M18 16v18M22 16v18M26 16v18M30 16v18" stroke="$L" stroke-width="1"/><path d="M17 24h14M17 29h14" stroke="$L" stroke-width="1.6" stroke-dasharray="2 2"/>',
                       '<path d="M16 8c2 4 5 6 8 6s6-2 8-6c-1 6-4 8-8 8s-7-2-8-8z" fill="$D"/>')
I['hoodie'] = ('<path d="M15 14L5 18 3 35l8 2 3-14v19h20V23l3 14 8-2-2-17-10-4z" fill="$C"/><path d="M15 14c-1-7 3-10 9-10s10 3 9 10c-2 4-5 6-9 6s-7-2-9-6z" fill="$D"/>'
               '<path d="M18 14c1 3 3 4 6 4s5-1 6-4c-1-3-3-5-6-5s-5 2-6 5z" fill="$K"/><rect x="17" y="30" width="14" height="8" rx="2" fill="$D"/><path d="M21 20v7M27 20v7" stroke="$L" stroke-width="1.8" stroke-linecap="round"/>'
               '<rect x="3" y="33" width="8.5" height="4" rx="1.5" fill="$D"/><rect x="36.5" y="33" width="8.5" height="4" rx="1.5" fill="$D"/>')
I['leather_jacket'] = ('<path d="M16 7L5 14l3 22 5-1v6h22v-6l5 1 3-22-11-7-8 6z" fill="$C"/><path d="M24 13v28" stroke="$D" stroke-width="2"/><path d="M16 7l8 6-6 8zM32 7l-8 6 6 8z" fill="$L"/>'
                       '<path d="M24 13v28" stroke="$L" stroke-width="1" stroke-dasharray="2 2"/><rect x="11" y="28" width="7" height="2.4" rx="1" fill="$D"/><rect x="30" y="28" width="7" height="2.4" rx="1" fill="$D"/><rect x="5" y="31" width="7" height="3" fill="$D"/><rect x="36" y="31" width="7" height="3" fill="$D"/>')
I['jeans'] = ('<path d="M13 5h22l3 38H27l-3-22-3 22H10z" fill="$C"/><rect x="13" y="5" width="22" height="6" fill="$D"/><path d="M24 11v10" stroke="$D" stroke-width="1.5"/>'
              '<path d="M14 11l5 3v5M34 11l-5 3v5" stroke="$L" stroke-width="1.5" fill="none"/><path d="M12 38h8M28 38h9" stroke="$D" stroke-width="2"/><rect x="21" y="5" width="6" height="3" fill="$L"/>')
I['work_gloves'] = ('<path d="M13 42V27l-5-7c-1-3 3-4 4-2l3 4V11c0-3 4-3 4 0V9c0-3 4-3 4 0v2c0-3 4-3 4 0v3c0-3 4-3 4 0v18l-2 11z" fill="$C"/><rect x="12" y="36" width="20" height="7" rx="1" fill="$D"/>'
                    '<path d="M16 30h13M17 14v10M22 12v12M27 15v9" stroke="$D" stroke-width="1.4" stroke-linecap="round"/><rect x="12" y="36" width="20" height="2" fill="$L"/>')
I['trainers'] = ('<path d="M5 20c4 2 8 1 10-3l12 9 13 2c3 1 4 3 4 6v1H5z" fill="$C"/><path d="M5 20c4 2 8 1 10-3l3 2c-2 4-8 5-13 3z" fill="$L"/><rect x="4" y="34" width="41" height="5" rx="2.5" fill="$L"/>'
                 '<path d="M18 22l-3 4M22 25l-3 4M26 28l-3 4" stroke="$D" stroke-width="1.8"/><path d="M32 30l10 2v2H32z" fill="$D"/>')
I['work_boots'] = ('<path d="M13 5h14v20l12 5c3 1 5 3 5 6v3H13z" fill="$C"/><rect x="11" y="38" width="35" height="5" rx="1.5" fill="$D"/><rect x="13" y="5" width="14" height="5" fill="$D"/>'
                   '<path d="M27 14h-6M27 19h-6M27 24h-6" stroke="$L" stroke-width="1.8"/><path d="M14 33h30" stroke="$L" stroke-width="1.5"/><path d="M31 31l12 4v3H31z" fill="$D"/>')
I['glasses'] = ('<rect x="4" y="19" width="17" height="14" rx="6" fill="$L"/><rect x="27" y="19" width="17" height="14" rx="6" fill="$L"/>'
                '<rect x="4" y="19" width="17" height="14" rx="6" fill="none" stroke="$C" stroke-width="3"/><rect x="27" y="19" width="17" height="14" rx="6" fill="none" stroke="$C" stroke-width="3"/>'
                '<path d="M21 24c2-2 4-2 6 0M4 22L1 17M44 22l3-5" stroke="$C" stroke-width="3" fill="none" stroke-linecap="round"/><path d="M8 24l4-2M31 24l4-2" stroke="$D" stroke-width="2" stroke-linecap="round"/>')
I['baseball_cap'] = ('<path d="M8 31c0-12 6-19 15-19s15 7 15 19z" fill="$C"/><path d="M24 12c-5 2-7 10-7 19" stroke="$D" stroke-width="1.5" fill="none"/><path d="M24 12c5 2 7 10 7 19" stroke="$D" stroke-width="1.5" fill="none"/>'
                     '<path d="M30 31h15c0-5-6-8-12-8z" fill="$D"/><circle cx="24" cy="11.5" r="2.4" fill="$D"/><rect x="8" y="29" width="30" height="3.5" fill="$L"/>')
I['beanie'] = ('<path d="M9 29c0-11 6-17 15-17s15 6 15 17z" fill="$C"/><rect x="7" y="27" width="34" height="11" rx="4" fill="$D"/><path d="M12 28v9M17 28v9M22 28v9M27 28v9M32 28v9M37 28v9" stroke="$C" stroke-width="1.6"/>'
                '<path d="M15 20c2-4 5-6 9-6" stroke="$L" stroke-width="2.4" fill="none" stroke-linecap="round"/>')
I['woolly_hat'] = ('<path d="M10 33c0-14 5-22 14-22s14 8 14 22z" fill="$C"/><rect x="8" y="31" width="32" height="10" rx="4" fill="$D"/><path d="M13 32v8M18 32v8M23 32v8M28 32v8M33 32v8M38 32v8" stroke="$C" stroke-width="1.6"/>'
                   '<path d="M14 25h20M15 19h18" stroke="$L" stroke-width="2.2"/><circle cx="24" cy="8" r="5.5" fill="$L"/><circle cx="22" cy="7" r="1.6" fill="$C"/><circle cx="26.5" cy="9" r="1.4" fill="$C"/>')
I['scarf'] = ('<path d="M6 12c6 8 30 8 36 0v9c-6 8-30 8-36 0z" fill="$C"/><path d="M6 12c6 8 30 8 36 0" stroke="$L" stroke-width="2.4" fill="none"/><path d="M6 17c6 6 30 6 36 0" stroke="$D" stroke-width="2" fill="none"/>'
              '<path d="M27 25h12l2 15H25z" fill="$C"/><path d="M26 31h14M25 36h16" stroke="$D" stroke-width="2.4"/><path d="M25 40v4M29 40v4M33 40v4M37 40v4M41 40v4" stroke="$L" stroke-width="1.8" stroke-linecap="round"/>')
I['dust_mask'] = ('<path d="M8 17c0-4 4-5 16-5s16 1 16 5v6c0 8-7 14-16 14S8 31 8 23z" fill="$C"/><path d="M8 17c0-4 4-5 16-5s16 1 16 5c-6 2-26 2-32 0z" fill="$L"/>'
                  '<path d="M16 16h16" stroke="$D" stroke-width="2"/><path d="M13 24c7 4 15 4 22 0M14 30c6 3 14 3 20 0" stroke="$D" stroke-width="1.8" fill="none"/>'
                  '<path d="M8 18C2 18 2 30 9 30M40 18c6 0 6 12-1 12" stroke="$D" stroke-width="2.4" fill="none" stroke-linecap="round"/>')
I['bandana'] = ('<path d="M6 12h36L24 41z" fill="$C"/><path d="M6 12h36l-3 5H9z" fill="$L"/><path d="M6 12L2 8M42 12l4-4" stroke="$D" stroke-width="3" stroke-linecap="round"/>'
                '<circle cx="24" cy="23" r="3" fill="$D"/><circle cx="17" cy="21" r="1.6" fill="$D"/><circle cx="31" cy="21" r="1.6" fill="$D"/><circle cx="20" cy="29" r="1.4" fill="$D"/><circle cx="28" cy="29" r="1.4" fill="$D"/><circle cx="24" cy="34" r="1.2" fill="$D"/>')
I['balaclava'] = ('<path d="M8 22c0-10 6-16 16-16s16 6 16 16v6l4 10H4l4-10z" fill="$C"/><path d="M4 38l4-10h32l4 10z" fill="$D"/><rect x="12" y="17" width="24" height="9" rx="4.5" fill="$K"/>'
                  '<path d="M12 12c4-4 20-4 24 0" stroke="$L" stroke-width="2.4" fill="none" stroke-linecap="round"/><path d="M10 33h28" stroke="$L" stroke-width="1.5"/>')
# ---------- Bags
I['tote_bag'] = ('<path d="M12 18c0-17 8-17 8 0M28 18c0-17 8-17 8 0" stroke="$D" stroke-width="3" fill="none"/><path d="M6 18h36v24a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2z" fill="$C"/>'
                 '<rect x="6" y="18" width="36" height="4" fill="$L"/><rect x="14" y="27" width="20" height="12" rx="2" fill="$D"/><rect x="16" y="29" width="16" height="3" rx="1" fill="$L"/>')
I['school_backpack'] = ('<path d="M19 10c0-7 10-7 10 0" stroke="$D" stroke-width="3" fill="none"/><path d="M10 20c0-8 6-11 14-11s14 3 14 11v19a4 4 0 0 1-4 4H14a4 4 0 0 1-4-4z" fill="$C"/>'
                        '<rect x="14" y="27" width="20" height="14" rx="3" fill="$D"/><path d="M15 31h18" stroke="$L" stroke-width="2"/><rect x="21" y="28" width="6" height="4" rx="1" fill="$L"/>'
                        '<rect x="5" y="30" width="7" height="12" rx="3" fill="$D"/><rect x="36" y="30" width="7" height="12" rx="3" fill="$D"/><path d="M14 20c4-3 16-3 20 0" stroke="$L" stroke-width="2.2" fill="none"/>')
I['hiking_pack'] = ('<path d="M16 7c0-4 4-5 8-5s8 1 8 5z" fill="$D"/><path d="M11 12c0-5 6-6 13-6s13 1 13 6v25H11z" fill="$C"/><path d="M11 12c0-5 6-6 13-6s13 1 13 6l-2 5H13z" fill="$L"/>'
                    '<rect x="15" y="25" width="18" height="12" rx="2.5" fill="$D"/><rect x="6" y="20" width="5" height="15" rx="2" fill="$D"/><rect x="37" y="20" width="5" height="15" rx="2" fill="$D"/>'
                    '<path d="M16 17v6M32 17v6" stroke="$D" stroke-width="2.6"/><rect x="8" y="38" width="32" height="7" rx="3.5" fill="$D"/><path d="M14 38v7M34 38v7" stroke="$L" stroke-width="2"/><path d="M19 28h10" stroke="$L" stroke-width="2"/>')
# ---------- Weapons
I['kitchen_knife'] = rot('<path d="M3 20h27l-6 12H3z" fill="$L"/><path d="M3 20h27" stroke="$D" stroke-width="1.5"/><rect x="30" y="20" width="14" height="7" rx="2.5" fill="$C"/><rect x="28" y="18" width="3.5" height="11" rx="1" fill="$D"/>'
                         '<circle cx="35" cy="23.5" r="1.2" fill="$D"/><circle cx="40" cy="23.5" r="1.2" fill="$D"/>')
I['frying_pan'] = rot('<rect x="24" y="21" width="21" height="6" rx="3" fill="$C"/><circle cx="46" cy="24" r="1.6" fill="$K"/><circle cx="15" cy="24" r="13" fill="$C"/><circle cx="15" cy="24" r="10" fill="$D"/>'
                      '<path d="M7 19a9 9 0 0 1 8-6" stroke="$L" stroke-width="2.2" fill="none" stroke-linecap="round"/><rect x="24" y="21" width="3" height="6" fill="$D"/>')
I['baseball_bat'] = rot('<path d="M2 20c0-3 2-4 5-4l22 4h14v8H29L7 32c-3 0-5-1-5-4z" fill="$C"/><rect x="34" y="20" width="9" height="8" fill="$D"/><rect x="43" y="18.5" width="3" height="11" rx="1.5" fill="$D"/><path d="M8 19l18 3" stroke="$L" stroke-width="2" stroke-linecap="round"/>')
I['wooden_plank'] = rot('<rect x="2" y="17" width="44" height="14" rx="1.5" fill="$C"/><rect x="2" y="17" width="44" height="4" fill="$L"/><rect x="2" y="17" width="5" height="14" fill="$D"/>'
                        '<path d="M12 25h14M30 27h12M16 22h10" stroke="$D" stroke-width="1.4" stroke-linecap="round"/><circle cx="12" cy="24" r="2.2" fill="$D"/><circle cx="40" cy="24" r="1.6" fill="$D"/>')
I['fire_axe'] = rot('<rect x="6" y="22" width="40" height="4.6" rx="2" fill="$C"/><path d="M8 11c6 3 10 3 14 2v22c-4-1-8-1-14 2-4-8-4-18 0-26z" fill="$L"/><path d="M22 18h8l6 5-6 4h-8z" fill="$D"/>'
                  '<path d="M8 11c-4 8-4 18 0 26" stroke="$D" stroke-width="2" fill="none"/><path d="M22 13v22" stroke="$D" stroke-width="2"/><rect x="38" y="21" width="3" height="6.6" fill="$D"/>')
I['log'] = ('<path d="M10 14h28v20H10z" fill="$C"/><path d="M10 14h28v5H10z" fill="$L"/><path d="M16 22h10M20 29h14" stroke="$D" stroke-width="1.6" stroke-linecap="round"/>'
            '<ellipse cx="38" cy="24" rx="6" ry="10" fill="$D"/><ellipse cx="38" cy="24" rx="4.6" ry="8.4" fill="#d9b98a"/><ellipse cx="38" cy="24" rx="2.6" ry="5" fill="none" stroke="#a9875a" stroke-width="1.1"/><circle cx="38" cy="24" r="0.9" fill="#a9875a"/>'
            '<ellipse cx="10" cy="24" rx="3" ry="10" fill="$D"/>')
I['hatchet'] = rot('<rect x="10" y="22" width="34" height="4.6" rx="2" fill="$C"/><path d="M4 12c5 3 9 3 13 2v20c-4-1-8-1-13 2-4-8-4-16 0-24z" fill="#b9bcc2"/><path d="M4 12c-4 8-4 16 0 24" stroke="#e6e8ea" stroke-width="2.2" fill="none"/>'
                   '<path d="M16 16.5h8v15h-8z" fill="#5c6068"/><rect x="38" y="21" width="3" height="6.6" fill="$D"/>')
I['pistol'] = ('<path d="M5 13h37v11H20l-1 3H9c-1 0-2-1-2-2z" fill="$C"/><path d="M5 13h37v3H5z" fill="$L"/><path d="M26 24h14v1.5H26z" fill="$D"/>'
               '<path d="M19 24h-5l-4 17c0 1 1 2 2 2h8c1 0 2-1 2-2z" fill="$D"/><path d="M22 27h13v4c0 1-1 2-2 2h-5" stroke="$D" stroke-width="2.4" fill="none"/>'
               '<rect x="41" y="12" width="3" height="4" fill="$D"/><path d="M15 17h12" stroke="$D" stroke-width="1.6"/><rect x="5" y="10.5" width="3" height="2.5" fill="$L"/><circle cx="23.5" cy="29.5" r="1.1" fill="$K"/>')
def rnd(x, y, w, h, brass=True):  # a pistol round standing at x
    return (f'<rect x="{x}" y="{y+h*.4}" width="{w}" height="{h*.6}" fill="$C"/><path d="M{x} {y+h*.4}v-{h*.1}c0-{h*.3} {w*.25}-{h*.3} {w*.5}-{h*.3}s{w*.5} 0 {w*.5} {h*.3}v{h*.1}z" fill="$L"/>'
            f'<rect x="{x-1}" y="{y+h-2.5}" width="{w+2}" height="2.5" rx=".8" fill="$D"/>')
I['pistol_rounds'] = rnd(8, 20, 9, 22) + rnd(19.5, 20, 9, 22) + rnd(31, 20, 9, 22) + '<rect x="6" y="40" width="36" height="3" rx="1.5" fill="$D"/>'
I['rifle_rounds'] = ('<path d="M20 4c3 0 6 6 6 13v1H14v-1c0-7 3-13 6-13z" fill="$L" transform="translate(4 0)"/><path d="M18 18h12v4l4 4v15H14V26l4-4z" fill="$C"/>'
                     '<rect x="13" y="38" width="22" height="5" rx="1.5" fill="$D"/><rect x="18" y="26" width="3" height="11" fill="$L"/>')
I['shotgun_shells'] = ''.join(f'<g transform="rotate({a} {x} 40)"><rect x="{x-7}" y="8" width="14" height="25" rx="2" fill="$C"/><rect x="{x-7}" y="8" width="14" height="4" rx="1.5" fill="$D"/><rect x="{x-7}" y="28" width="14" height="13" rx="2" fill="$L"/><rect x="{x-8}" y="39" width="16" height="3.5" rx="1.5" fill="$D"/><rect x="{x-4}" y="14" width="3" height="12" fill="$L"/></g>' for x, a in ((15, -8), (33, 8)))
I['shotgun'] = rot('<path d="M1 24c4-4 8-4 14-4h7v7H12l-3 7H2z" fill="$C"/><rect x="22" y="19" width="23" height="4.6" rx="1" fill="$L"/><rect x="22" y="24" width="23" height="4" rx="1" fill="$D"/><rect x="26" y="26" width="12" height="5" rx="2" fill="$C"/>'
                    '<path d="M22 28v5" stroke="$D" stroke-width="2"/><rect x="44" y="18" width="3" height="11" rx="1" fill="$L"/><path d="M13 24h8" stroke="$D" stroke-width="1.4"/>')
# ---------- Junk
I['mug'] = ('<path d="M9 12h23v25a4 4 0 0 1-4 4H13a4 4 0 0 1-4-4z" fill="$C"/><path d="M32 17h4a5.5 5.5 0 0 1 0 11h-4" stroke="$C" stroke-width="3.5" fill="none"/><rect x="9" y="12" width="23" height="5" rx="1" fill="$L"/>'
            '<ellipse cx="20.5" cy="12" rx="11.5" ry="2" fill="$D"/><rect x="11" y="20" width="4" height="17" fill="$L"/><path d="M15 6c-2 2 2 3 0 5M21 5c-2 2 2 3 0 6" stroke="$L" stroke-width="1.6" fill="none" stroke-linecap="round"/>')
I['magazine'] = (rot('<rect x="9" y="5" width="30" height="38" rx="2" fill="$C"/><rect x="9" y="5" width="5" height="38" fill="$D"/><rect x="17" y="9" width="19" height="7" fill="$L"/>'
                     '<circle cx="27" cy="29" r="7" fill="$L"/><path d="M17 38h19M17 41h12" stroke="$D" stroke-width="1.6"/><path d="M22 28c3-4 5 4 9 0" stroke="$D" stroke-width="1.6" fill="none"/>', 8))
I['tv_remote'] = rot('<rect x="14" y="3" width="21" height="42" rx="4" fill="$C"/><rect x="14" y="3" width="21" height="7" rx="3" fill="$D"/><circle cx="19" cy="6.5" r="1.6" fill="$L"/>'
                     + ''.join(f'<circle cx="{x}" cy="{y}" r="2.2" fill="$L"/>' for x in (19.5, 24.5, 29.5) for y in (16, 22, 28))
                     + '<rect x="19" y="35" width="11" height="5" rx="2" fill="$D"/>', 28)
I['picture_frame'] = ('<rect x="5" y="7" width="38" height="34" rx="2" fill="$C"/><rect x="10" y="12" width="28" height="24" fill="$D"/><rect x="12" y="14" width="24" height="20" fill="$L"/>'
                      '<path d="M12 34l8-11 5 6 4-5 7 10z" fill="$C"/><circle cx="31" cy="20" r="3" fill="$D"/><path d="M5 7l5 5M43 7l-5 5M5 41l5-5M43 41l-5-5" stroke="$L" stroke-width="1.5"/>')
I['pen'] = rot('<path d="M2 24l6-3.5h4v7H8z" fill="$L"/><rect x="12" y="20" width="22" height="8" rx="2" fill="$C"/><rect x="12" y="20" width="22" height="3" fill="$L"/><rect x="34" y="19" width="10" height="10" rx="3" fill="$D"/><path d="M34 22h8" stroke="$L" stroke-width="2"/><rect x="36" y="16" width="8" height="3" rx="1" fill="$L"/>')
I['baking_tray'] = ('<rect x="3" y="10" width="42" height="28" rx="4" fill="$C"/><rect x="7" y="14" width="34" height="20" rx="2" fill="$D"/><rect x="3" y="10" width="42" height="4" rx="2" fill="$L"/><rect x="3" y="34" width="42" height="4" rx="2" fill="$L"/>'
                    '<path d="M11 20h26M11 28h26" stroke="$C" stroke-width="1.6"/><rect x="1" y="21" width="5" height="6" rx="2" fill="$D"/><rect x="42" y="21" width="5" height="6" rx="2" fill="$D"/>')
I['alarm_clock'] = ('<path d="M6 14a8 7 0 0 1 14-5zM42 14a8 7 0 0 0-14-5z" fill="$L"/><circle cx="24" cy="26" r="16" fill="$C"/><circle cx="24" cy="26" r="12" fill="$L"/>'
                    '<path d="M24 26V17M24 26l6 4" stroke="$D" stroke-width="2.6" stroke-linecap="round"/><circle cx="24" cy="26" r="1.8" fill="$D"/><path d="M12 42l-3 3M36 42l3 3" stroke="$D" stroke-width="3.5" stroke-linecap="round"/>'
                    '<path d="M24 10v3M24 39v3M8 26h3M37 26h3" stroke="$D" stroke-width="1.6"/><rect x="21" y="6" width="6" height="3" rx="1" fill="$D"/>')
I['crackers'] = ('<path d="M7 14h28v26H7z" fill="$C"/><path d="M7 14l5-6h28l-5 6z" fill="$L"/><path d="M35 14l5-6v26l-5 6z" fill="$D"/>'
                 '<rect x="11" y="19" width="20" height="16" rx="2" fill="$D"/><rect x="13" y="21" width="16" height="12" rx="1.5" fill="$L"/>'
                 + ''.join(f'<circle cx="{x}" cy="{y}" r="1.1" fill="$D"/>' for x in (17, 21, 25) for y in (25, 29)))
I['sewing_kit'] = ('<circle cx="24" cy="25" r="18" fill="$C"/><circle cx="24" cy="25" r="18" fill="none" stroke="$D" stroke-width="2"/><circle cx="24" cy="25" r="13" fill="$D"/>'
                   '<circle cx="24" cy="25" r="10" fill="$L"/><circle cx="24" cy="25" r="7" fill="none" stroke="$C" stroke-width="1.6"/>'
                   '<circle cx="21" cy="22" r="1.7" fill="$K"/><circle cx="27" cy="22" r="1.7" fill="$K"/><circle cx="21" cy="28" r="1.7" fill="$K"/><circle cx="27" cy="28" r="1.7" fill="$K"/>'
                   '<path d="M21 22l6 6M27 22l-6 6" stroke="$D" stroke-width="1.2"/><path d="M8 10c4-3 7-3 9 0" stroke="$L" stroke-width="2.4" fill="none" stroke-linecap="round"/>')
I['bed_sheet'] = ('<path d="M4 34l5-5h34l-5 5z" fill="$L"/><rect x="4" y="34" width="34" height="8" rx="1.5" fill="$D"/><path d="M38 34l5-5v8l-5 5z" fill="$D"/>'
                  '<rect x="6" y="25" width="32" height="9" rx="1.5" fill="$C"/><path d="M6 25l5-6h32l-5 6z" fill="$L"/><path d="M38 25l5-6v10l-5 5z" fill="$D"/>'
                  '<rect x="6" y="28" width="32" height="2.5" fill="$D"/><path d="M11 22.5h27" stroke="$D" stroke-width="1.4"/><path d="M14 36.5h20" stroke="$L" stroke-width="1.4"/>')
I['plastic_bag'] = ('<path d="M17 18c0-14 14-14 14 0" stroke="$L" stroke-width="3" fill="none"/><path d="M11 17h26l3 25H8z" fill="$C"/><path d="M11 17h26l1 6H10z" fill="$L"/>'
                    '<path d="M18 24l-2 18M30 24l3 18" stroke="$D" stroke-width="2"/><path d="M24 27v15" stroke="$L" stroke-width="2"/><path d="M8 42l32 0-.3-2H8.3z" fill="$D"/>')
I['duffel_bag'] = ('<path d="M12 18c4-10 20-10 24 0" stroke="$L" stroke-width="3.5" fill="none"/><rect x="3" y="17" width="42" height="22" rx="10" fill="$C"/><rect x="3" y="17" width="42" height="7" rx="4" fill="$L"/>'
                   '<path d="M3 26a10 10 0 0 1 8-9v22a10 10 0 0 1-8-9z" fill="$D"/><path d="M45 26a10 10 0 0 0-8-9v22a10 10 0 0 0 8-9z" fill="$D"/>'
                   '<path d="M15 24h18" stroke="$D" stroke-width="2"/><rect x="21" y="26" width="6" height="7" rx="1.5" fill="$L"/>')
I['crowbar'] = rot('<path d="M42 22H13c-5 0-8 3-8 8v2" stroke="$C" stroke-width="6.5" fill="none" stroke-linecap="round"/><path d="M38 22h8" stroke="$L" stroke-width="6.5"/><path d="M4 33l1 5 4-3z" fill="$L"/>'
                   '<path d="M16 22h14" stroke="$D" stroke-width="1.6" stroke-dasharray="3 3"/><path d="M43 17.5l4 4.5-4 4.5z" fill="$L"/>')
I['pistol_magazine'] = rot('<path d="M15 12h18l2 31H13z" fill="$C"/><path d="M15 12h18v5H15z" fill="$L"/><path d="M13 39h22l1 4H12z" fill="$D"/>'
                           '<path d="M16 12c0-5 2-8 4-8s4 3 4 8zM24 12c0-5 2-8 4-8s4 3 4 8z" fill="$L"/><circle cx="24" cy="24" r="1.7" fill="$K"/><circle cx="24" cy="29" r="1.7" fill="$K"/><circle cx="24" cy="34" r="1.7" fill="$K"/><path d="M19 18v20" stroke="$D" stroke-width="1.5"/>', 12)
I['umbrella'] = rot('<path d="M17 24H5" stroke="$L" stroke-width="2.4"/><path d="M5 24c-3 0-4 7 1 8s6-3 6-5" stroke="$C" stroke-width="3.5" fill="none" stroke-linecap="round" transform="translate(0 0)"/>'
                    '<path d="M15 24c1-5 5-6 9-6 8 0 14 3 21 6-7 3-13 6-21 6-4 0-8-1-9-6z" fill="$C"/><path d="M15 24c1-5 5-6 9-6 8 0 14 3 21 6-8-1-14-3-30 0z" fill="$L"/>'
                    '<path d="M24 18c-1 4-1 8 0 12M31 19.5c-1 3-1 6 0 9M38 22c-1 1-1 3 0 4" stroke="$D" stroke-width="1.6" fill="none"/><rect x="27" y="22" width="3.5" height="4" fill="$D"/><path d="M45 24l3 0" stroke="$L" stroke-width="2.4"/>')
I['cushion'] = ('<path d="M6 8Q24 3 42 8Q47 24 42 40Q24 45 6 40Q1 24 6 8z" fill="$C"/><path d="M6 8Q24 3 42 8L40 14Q24 10 8 14z" fill="$L"/><path d="M6 40Q24 45 42 40L40 35Q24 39 8 35z" fill="$D"/>'
                '<path d="M6 8l5 3-3 4zM42 8l-5 3 3 4zM6 40l5-3-3-4zM42 40l-5-3 3-4z" fill="$D"/><circle cx="24" cy="24" r="3.6" fill="$D"/><circle cx="24" cy="24" r="1.5" fill="$L"/>'
                '<path d="M24 24L14 17M24 24l10-7M24 24l-10 7M24 24l10 7" stroke="$D" stroke-width="1.3"/>')

I['claw_hammer'] = rot('<rect x="18" y="21.5" width="27" height="5.5" rx="2.5" fill="$C"/><rect x="9" y="9" width="11" height="24" rx="2" fill="$L"/><rect x="9" y="7" width="11" height="4" rx="1.5" fill="$D"/>'
                       '<path d="M9 33h11v2c-4 0-8 1-10 4-1-2-1-4-1-6z" fill="$L"/><rect x="17" y="9" width="3" height="26" fill="$D"/><path d="M11 15v14" stroke="$D" stroke-width="1.5"/>')
I['golf_club'] = rot('<rect x="14" y="23" width="30" height="2.4" fill="$L"/><rect x="33" y="21.5" width="12" height="5.4" rx="2.7" fill="$C"/><path d="M2 21h11l3 3-1 10H2z" fill="$L"/>'
                     '<path d="M2 28h13" stroke="$D" stroke-width="1.6"/><path d="M13 21l3 3-1 10" stroke="$D" stroke-width="1.6" fill="none"/>')
I['hunting_rifle'] = rot('<path d="M1 21l15 1v7H6l-1 5H1z" fill="$C"/><rect x="15" y="20" width="14" height="8" rx="1" fill="$D"/><rect x="29" y="22" width="17" height="3.8" fill="$L"/><rect x="44" y="21" width="3" height="5.6" fill="$D"/>'
                         '<rect x="17" y="12" width="19" height="5" rx="2" fill="$D"/><rect x="14" y="10.5" width="5" height="8" rx="2" fill="$L"/><rect x="34" y="10.5" width="5" height="8" rx="2" fill="$L"/><path d="M21 17v4M31 17v4" stroke="$D" stroke-width="2"/><path d="M20 28l1 4" stroke="$D" stroke-width="2"/>')

def main():
    rows = [l.rstrip('\n').split('\t') for l in open(os.path.join(HERE, 'items.tsv')) if l.strip()]
    ids = [r[0] for r in rows]
    missing = [i for i in ids if i not in I]
    extra = [k for k in I if k not in ids]
    assert not missing and not extra, (missing, extra)
    os.makedirs(os.path.join(HERE, 'svg'), exist_ok=True)
    for iid, name, cat, col in rows:
        C, D, L = tones(col)
        body = I[iid].replace('$C', C).replace('$D', D).replace('$L', L).replace('$K', K)
        # fill="..." is written as fill="$C" so tokens are whole-attr; stroke tokens likewise
        svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">{body}</svg>\n'
        open(os.path.join(HERE, 'svg', iid + '.svg'), 'w').write(svg)
    print(len(rows), 'svgs')
main()
