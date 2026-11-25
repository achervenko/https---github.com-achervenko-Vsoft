from pathlib import Path
import math, io, json, collections, textwrap
import ezdxf
from ezdxf import bbox
from ezdxf.math import Matrix44, Vec3, BoundingBox2d
from ezdxf.fonts import fonts
import pymupdf as fitz
from PIL import Image
from fontTools.ttLib import TTFont

import sys
ROOT, OUT, REFERENCE = map(Path,sys.argv[1:4])
OUT.mkdir(exist_ok=True,parents=True)
pdf=fitz.open(ROOT/'source.pdf')
pages=json.loads((ROOT/'pages.json').read_text())
source_total=pages['total']
source_numbers=pages['numbers']
native=ezdxf.readfile(ROOT/'native.dxf')
ref=ezdxf.readfile(REFERENCE)
doc=ezdxf.new('R2018')
doc.units=4
doc.header['$INSUNITS']=4
doc.header['$MEASUREMENT']=1
doc.header['$LWDISPLAY']=False
doc.styles.new('POGA_ISO',dxfattribs={'font':'isocpeur.ttf'})
iso=fonts.make_font('C:/Windows/Fonts/isocpeur.ttf',cap_height=1)
for name,color,lw in [('FRAME',7,35),('FRAME_TEXT',7,18),('WIRES',6,18),('SYMBOLS',7,18),('TEXT',5,18),('TABLES',7,18),('FILLS',7,13),('LOGO',7,13)]:
    doc.layers.new(name,dxfattribs={'color':color,'lineweight':lw})

def metadata(p):
    spans=[sp for b in p.get_text('dict')['blocks'] if b['type']==0 for l in b['lines'] for sp in l['spans'] if sp['origin'][1]>700]
    def first(pred, default=''):
        return next((sp['text'].strip() for sp in spans if pred(*sp['origin'],sp['text'])),default)
    return (first(lambda x,y,t:t.startswith('RT-')),
            ' '.join(sp['text'].strip() for sp in sorted(spans,key=lambda s:s['origin'][1]) if 510<sp['origin'][0]<560),
            first(lambda x,y,t:'РусАгро' in t),
            first(lambda x,y,t:'Система газоанализации' in t))
meta=[metadata(p) for p in pdf]
titles=[m[1] or f'Лист {n}' for m,n in zip(meta,source_numbers)]
frame=doc.blocks.new('POGA_A3_FRAME')
source_frame=ref.entitydb['229DD']
origin=Vec3(-2194.464723450034,-795.0950713227214,0)
move=Matrix44.translate(-origin.x,-origin.y,0)
for ent in source_frame.virtual_entities():
    e=ent.copy()
    e.xdata=None
    e.reactors=None
    e.appdata=None
    if e.has_extension_dict: e.discard_extension_dict()
    e.transform(move)
    e.dxf.layer='FRAME_TEXT' if e.dxftype()=='TEXT' else 'FRAME'
    e.dxf.color=256
    e.dxf.linetype='Continuous'
    e.dxf.lineweight=-1
    if e.dxftype()=='TEXT':
        e.dxf.style='POGA_ISO'
        e.dxf.text=e.dxf.text.replace('%%d','№')
    frame.add_entity(e)

def text(target, value, x, y, h, layer='FRAME_TEXT', max_width=None, rotation=0):
    if not value.strip(): return None
    width=1
    if max_width:
        natural=iso.text_width(value)*h
        width=min(1,max_width/natural) if natural else 1
    return target.add_text(value,dxfattribs={'insert':(x,y),'height':h,'style':'POGA_ISO','width':width,'rotation':rotation,'layer':layer})

# Attribute locations come from the reference frame; project values come from the PDF.
fields={
    '!ШИФР':('CODE',3.3,112), '!НОМЕР':('SHEET',3,9),
    'КОЛИЧЕСТВО_ЛИСТОВ':('TOTAL',3,10),
    'НАИМЕНОВАНИЕ1':('CUSTOMER',3,112),
    'НАИМЕНОВАНИЕ2':('PROJECT',3,112),
    'СООРУЖЕНИЕ1':('SYSTEM',2.7,72),
    'СООРУЖЕНИЕ2':('DESCRIPTION',2.7,72),
    'НАИМЕНОВАНИЕ_ЛИСТА1':('TITLE1',2.7,72),
    'НАИМЕНОВАНИЕ_ЛИСТА2':('TITLE2',2.7,72),
}
for a in source_frame.attribs:
    if a.dxf.tag not in fields: continue
    name,h,w=fields[a.dxf.tag]
    pos=Vec3(a.dxf.insert)-origin
    # Use the reference field centre, with explicit alignment for reliable fonts.
    if name in ['CODE','CUSTOMER','PROJECT']:
        pos=Vec3(297.5,pos.y,0)
    elif name in ['SYSTEM','DESCRIPTION','TITLE1','TITLE2']:
        pos=Vec3(297.5,pos.y,0)
    attr=frame.add_attdef(name,insert=pos,text='',dxfattribs={'height':h,'style':'POGA_ISO','layer':'FRAME_TEXT'})
    attr.dxf.width=.8

# Main content is kept above the 55 mm reference title block.
S=228/700
X=20+(395-1127*S)/2
Y=63
cap={}
for p in pdf:
    for xref,ext,_,fontname,*_ in p.get_fonts():
        if fontname in cap: continue
        try:
            tf=TTFont(io.BytesIO(pdf.extract_font(xref)[3]))
            gly=tf['glyf'][tf.getBestCmap()[ord('H')]]
            cap[fontname]=gly.yMax/tf['head'].unitsPerEm
        except Exception: cap[fontname]=.685

geometries=[[] for _ in pdf]
excluded=[collections.Counter() for _ in pdf]
for e in native.modelspace():
    if e.dxftype() in ['MTEXT','TEXT','IMAGE']: continue
    box=bbox.extents([e])
    if not box.has_data: continue
    idx=round((box.center.x-7.8)/20)
    if not 0<=idx<len(pdf): continue
    # Original EPLAN footer is replaced by the reference frame.
    if box.extmin.y < (762-700)/72-0.005:
        excluded[idx][e.dxftype()]+=1
        continue
    geometries[idx].append(e)

import re

def segment_key(a,b):
    return tuple(sorted(((round(a[0],2),round(a[1],2)),(round(b[0],2),round(b[1],2)))))

def dash_patterns(page):
    segments, curves, arcs = {}, {}, {}
    for path in page.get_drawings():
        match=re.match(r'\[([^]]+)\]\s*(.*)',path.get('dashes') or '')
        if not match: continue
        pattern=tuple(float(v)*S for v in match[1].split())
        if not pattern: continue
        if pattern not in line_types:
            name=f'PDF_DASH_{len(line_types)+1:03d}'
            doc.linetypes.new(name,dxfattribs={'description':'PDF stroke pattern','pattern':[sum(pattern),*(v*(-1 if i%2 else 1) for i,v in enumerate(pattern))]})
            line_types[pattern]=name
        name=line_types[pattern]
        for item in path['items']:
            if item[0]=='l' and max(item[1].y,item[2].y)<700.2:
                segments[segment_key(item[1],item[2])]=name
        if path['rect'].y1<700.2 and any(item[0]!='l' for item in path['items']):
            curves[tuple(round(v,1) for v in path['rect'])]=name
            if all(item[0]=='c' for item in path['items']):
                k=segment_key(path['items'][0][1],path['items'][-1][-1])
                arcs[k]=tuple(round(v,1) for v in path['rect'])
    return segments,curves,arcs

line_types={}
report=[]
for i,p in enumerate(pdf):
    shift=i*440
    previous_handles={e.dxf.handle for e in doc.modelspace()}
    code,_,customer,system=meta[i]
    title=titles[i]
    vals={'CODE':code,'SHEET':str(source_numbers[i]),'TOTAL':str(source_total),'CUSTOMER':customer,'PROJECT':'','SYSTEM':system,'DESCRIPTION':'','TITLE1':title,'TITLE2':''}
    import textwrap
    vals['TITLE1']='\\P'.join(textwrap.wrap(title, width=52, break_long_words=False))
    ins=doc.modelspace().add_blockref(frame.name,(shift,0),dxfattribs={'layer':'FRAME'})
    body=doc.modelspace()
    for att in frame.query('ATTDEF'):
        tag=att.dxf.tag
        if not vals[tag]: continue
        x,y,_=att.dxf.insert
        h=att.dxf.height
        w=10 if tag in ('TOTAL','SHEET') else 64 if tag in ('TITLE1','TITLE2','SYSTEM','DESCRIPTION') else 110
        value=vals[tag]
        factor=min(.9,w/max(1,max(iso.text_width(v)*h for v in value.split('\\P'))))
        body.add_mtext('{\\W%.5f;%s}'%(factor,value),dxfattribs={'insert':(shift+x,y+h*1.15),'char_height':h,'style':'POGA_ISO','layer':'FRAME_TEXT','attachment_point':1})
    # Native PDF import supplies CAD circles, arcs, polylines and hatches.
    k=72*S
    transform=Matrix44.chain(Matrix44.translate(-i*20,0,0),Matrix44.scale(k),Matrix44.translate(shift+X,Y-(762-700)*S,0))
    counts=collections.Counter()
    dashed, dashed_curves, dashed_arcs=dash_patterns(p)
    matched, matched_curves=set(),set()
    def from_cad(point):
        return ((point[0]-i*20)*72,762-point[1]*72)

    for e in geometries[i]:
        c=e.copy()
        c.xdata=None
        c.reactors=None
        c.appdata=None
        if c.has_extension_dict: c.discard_extension_dict()
        c.transform(transform)
        b=bbox.extents([c])
        typ=c.dxftype()
        layer='TABLES' if any(k in titles[i].lower() for k in ('содержание','перечень','спецификация')) else 'SYMBOLS'
        if typ in ['HATCH','SOLID']: layer='FILLS'
        elif layer!='TABLES' and source_numbers[i]>8 and typ in ['LINE','LWPOLYLINE']:
            # Long orthogonal lines are the schematic connection network.
            if (b.size.x<.02 and b.size.y>7) or (b.size.y<.02 and b.size.x>7): layer='WIRES'
        c.dxf.layer=layer
        c.dxf.color=256
        c.dxf.lineweight=-1
        c.dxf.linetype='Continuous'
        added=False
        if typ=='LWPOLYLINE':
            pts=list(e.get_points('xyb'))
            transformed=list(c.get_points('xyb'))
            if e.closed:
                pts.append(pts[0])
                transformed.append(transformed[0])
            keys=[segment_key(from_cad(a),from_cad(b)) for a,b in zip(pts,pts[1:])]
            if any((key in dashed_arcs if a[2] else key in dashed) for a,key in zip(pts,keys)):
                added=True
                for a,b,key in zip(transformed,transformed[1:],keys):
                    if not a[2] and key in dashed:
                        matched.add(key)
                        body.add_line(a[:2],b[:2],dxfattribs={'layer':layer,'linetype':dashed[key]})
                        counts['LINE']+=1
                    else:
                        attributes={'layer':layer}
                        if a[2] and key in dashed_arcs:
                            curve=dashed_arcs[key]
                            matched_curves.add(curve)
                            attributes['linetype']=dashed_curves[curve]
                        body.add_lwpolyline([a,(b[0],b[1],0)],format='xyb',dxfattribs=attributes)
                        counts['LWPOLYLINE']+=1
        if not added:
            if dashed_curves:
                original=bbox.extents([e])
                x1,y2=from_cad(original.extmin)
                x2,y1=from_cad(original.extmax)
                key=tuple(round(v,1) for v in (x1,y1,x2,y2))
                if key in dashed_curves:
                    c.dxf.linetype=dashed_curves[key]
                    matched_curves.add(key)
            body.add_entity(c)
            counts[typ]+=1
    spans=0
    text_values=[]
    multiline=0
    for block in p.get_text('dict')['blocks']:
        if block['type']!=0: continue
        rows=[]
        for line in block['lines']:
            angle=math.degrees(math.atan2(-line['dir'][1],line['dir'][0]))
            for sp in line['spans']:
                if sp['bbox'][3]>700.2 or not sp['text'].strip(): continue
                value=sp['text'].replace('\u200e','').replace('\u200f','')
                ox,oy=sp['origin']
                h=sp['size']*cap.get(sp['font'],.685)*S
                quad=fitz.recover_quad(line['dir'],sp)
                desired=abs(quad.ur-quad.ul)*S
                natural=iso.text_width(value)*h
                factor=desired/natural if natural else 1
                escaped=value.replace('\\','\\\\').replace('{','\\{').replace('}','\\}')
                rows.append((ox,oy,h,angle,'{\\W%.6f;%s}'%(factor,escaped),value))
                spans+=1
                text_values.append(value)
        groups=[]
        for row in rows:
            prev=groups[-1][-1] if groups else None
            gap=(row[1]-prev[1])*S if prev else 0
            if prev and abs(row[3])<.01 and abs(prev[3])<.01 and abs(row[0]-prev[0])*S<.15 and abs(row[2]-prev[2])<.03 and 0.95<gap/row[2]<2.5 and (len(groups[-1])==1 or abs(gap-(prev[1]-groups[-1][-2][1])*S)<.12):
                groups[-1].append(row)
            else: groups.append([row])
        for group in groups:
            ox,oy,h,angle,_,_=group[0]
            rad=math.radians(angle)
            rise=h
            e=body.add_mtext('\\P'.join(row[4] for row in group),dxfattribs={'insert':(shift+X+ox*S-rise*math.sin(rad),Y+(700-oy)*S+rise*math.cos(rad)),'rotation':angle,'char_height':h,'style':'POGA_ISO','layer':'TEXT','attachment_point':1})
            if len(group)>1:
                multiline+=1
                e.dxf.line_spacing_style=2
                e.dxf.line_spacing_factor=max(.25,min(4,(group[1][1]-oy)*S/(h*5/3)))
    # The GEA logo is the PDF's only raster content. Trace dark pixel runs into
    # filled CAD rectangles, so the delivered drawing has no image dependencies.
    logo_rects=0
    for img in p.get_images():
        for rect in p.get_image_rects(img[0]):
            if rect.y1>700: continue
            im=Image.open(io.BytesIO(pdf.extract_image(img[0])['image'])).convert('L')
            pix=im.load()
            for yy in range(im.height):
                xx=0
                while xx<im.width:
                    if pix[xx,yy]>170: xx+=1; continue
                    start=xx
                    while xx<im.width and pix[xx,yy]<=170: xx+=1
                    x1=shift+X+(rect.x0+start/im.width*rect.width)*S
                    x2=shift+X+(rect.x0+xx/im.width*rect.width)*S
                    y1=Y+(700-rect.y0-(yy+1)/im.height*rect.height)*S
                    y2=Y+(700-rect.y0-yy/im.height*rect.height)*S
                    body.add_solid([(x1,y1),(x2,y1),(x1,y2),(x2,y2)],dxfattribs={'layer':'LOGO'})
                    logo_rects+=1
    # Named groups make selecting an entire sheet possible without trapping text
    # inside a page-sized block.
    sheet_entities=[e for e in body if e.dxf.handle not in previous_handles]
    doc.groups.new(name=f'SHEET_{source_numbers[i]:03d}',description=titles[i],selectable=False).set_data(sheet_entities)
    report.append({'sheet':source_numbers[i],'title':titles[i],'geometry':dict(counts),'text_spans':spans,'logo_solids':logo_rects,'multiline':multiline,'dash_segments_restored':len(matched),'dash_curves_restored':len(matched_curves),'dash_unmatched':len(set(dashed)-matched)+len(set(dashed_curves)-matched_curves),'replacement_chars':sum(t.count(chr(65533)) for t in text_values),'excluded_footer':dict(excluded[i])})
    if not counts or not spans: print('REVIEW sheet',source_numbers[i],flush=True)

doc.set_modelspace_vport(height=340,center=(210,145))
old_layouts=[name for name in doc.layouts.names() if name!='Model']
for i in range(len(pdf)):
    paper=doc.layouts.new(f'{source_numbers[i]:03d}')
    paper.page_setup(size=(420,297),margins=(0,0,0,0),units='mm',device='DWG To PDF.pc3')
    paper.dxf_layout.dxf.current_style_sheet='monochrome.ctb'
    paper.dxf_layout.dxf.paper_size='ISO_full_bleed_A3_(420.00_x_297.00_MM)'
    paper.dxf_layout.dxf.taborder=i+1
    vp=paper.add_viewport(center=(210,148.5),size=(420,297),view_center_point=(i*440+210,148.5),view_height=297)
    vp.dxf.flags=vp.dxf.flags | 16384
    vp.dxf.layer='Defpoints'
    for v in paper.query('VIEWPORT'):
        if v.dxf.id==1: v.dxf.layer='0'
for name in old_layouts: doc.layouts.delete(name)
doc.header['$TILEMODE']=1
audit=doc.audit()
print('AUDIT',len(audit.errors),len(audit.fixes),flush=True)
doc.saveas(OUT/'drawing.dxf')
(ROOT/'build_report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print('SAVED',flush=True)



