from pathlib import Path
import json,html
from PIL import Image
import argparse
parser=argparse.ArgumentParser(description='Render annotated HTML from localized demo captures')
parser.add_argument('--capture-root',type=Path,required=True)
root=parser.parse_args().capture_root.resolve()
copy=json.loads((Path(__file__).with_name('localized-captions.json')).read_text(encoding='utf8'))
scene_map={'remote-character':'candidates'}
css='''*{box-sizing:border-box}html,body{margin:0;background:#101216;color:#eee;font-family:"Segoe UI","Microsoft YaHei UI","Microsoft YaHei",Arial,sans-serif}body.card{width:1500px;padding:32px}header{display:flex;justify-content:space-between;align-items:center;margin-bottom:25px;gap:20px}small{color:#c9a24a;letter-spacing:2px;text-transform:uppercase;font-size:13px;font-weight:600}h1{font-size:32px;line-height:1.25;margin:8px 0 0;letter-spacing:-.4px;max-width:1190px}header>span{flex-shrink:0;font-size:15px;color:#a5a8b0;border:1px solid #34363d;border-radius:30px;padding:10px 16px}.layout{display:grid;grid-template-columns:1100px 312px;gap:24px;align-items:start}.shot{position:relative;border:1px solid #3b3530;border-radius:12px;overflow:hidden;background:#0e0f11;box-shadow:0 18px 48px #0005}.shot img{display:block;width:100%;height:auto}.mark{position:absolute;border:1.5px solid #dfb65d99;border-radius:8px;box-shadow:0 0 0 1px #0006}.mark b,.note b{background:#dfb65d;color:#17140d;border-radius:50%;display:grid;place-items:center;font-size:15px;font-weight:800;width:28px;height:28px}.mark b{position:absolute;left:-12px;top:-12px;border:2px solid #121316}.note{background:#191c21;border:1px solid #33353c;border-radius:12px;padding:20px;margin-bottom:16px}.note h2{font-size:20px;line-height:1.35;margin:12px 0 8px;overflow-wrap:normal;hyphens:none}.note p{font-size:17px;line-height:1.55;color:#b7bac2;margin:0;overflow-wrap:normal;hyphens:none}footer{color:#858a95;font-size:13px;margin-top:20px;display:flex;justify-content:space-between}'''
(root/'gallery').mkdir(exist_ok=True)
(root/'gallery/style.css').write_text(css,encoding='utf8')
def rect(x,y,w,h):return dict(x=x,y=y,width=w,height=h)
def union(items):
 items=[x for x in items if x and x['width']>0 and x['height']>0]
 assert items
 left=min(x['x'] for x in items);top=min(x['y'] for x in items)
 return rect(left,top,max(x['x']+x['width'] for x in items)-left,max(x['y']+x['height'] for x in items)-top)
def markers(slug,d):
 cards=[x for x in d['cards'] if x['height']>0];buttons=[b for b in d['buttons'] if b['width']>0 and b['height']>0]
 if slug=='setup':return cards[:2]
 if slug=='overview':return [cards[0],rect(cards[1]['x'],cards[1]['y'],cards[1]['width'],buttons[0]['y']-cards[1]['y']-18),union(buttons[:5])]
 if slug=='bots':
  c=cards[0];add=buttons[0];split=add['y']+add['height']+20
  return [rect(c['x']+10,c['y']+72,c['width']-20,split-c['y']-72),rect(c['x']+10,split,c['width']-20,c['y']+c['height']-split-12),rect(cards[1]['x'],cards[1]['y']-110,cards[1]['width'],cards[1]['height']+110)]
 if slug=='modules':return [[b for b in buttons if b['role']=='switch'][1],next(b for b in buttons if b['text'].strip() and b['role']!='switch')]
 if slug=='client':return [cards[1],union([b for b in buttons if b['text'].strip()][-2:])]
 if slug=='backups':return cards[:3]
 if slug=='friends':return [cards[0],union(cards[1:4]),cards[4]]
 if slug=='realms':return [d['filters'],d['table']]
 if slug=='portable':return [cards[0],union(cards[1:3])]
 if slug=='remote-character':
  c=d['characters'];lst=d['characterList'];return [rect(c['x'],c['y'],c['width'],lst['y']-c['y']-5),lst]
 raise ValueError(slug)
manifest=[]
labels={'ru':'Русский','de':'Deutsch','fr':'Français','es':'Español','zh':'简体中文'}
for lang,cards in copy.items():
 folder=root/'gallery'/lang;folder.mkdir(exist_ok=True)
 for index,(slug,content) in enumerate(cards.items(),1):
  scene=scene_map.get(slug,slug)
  raw=root/'raw'/lang/(scene+'.jpg')
  w,h=Image.open(raw).size
  if slug=="overview":h=1040
  data=json.loads((root/'raw'/lang/(scene+'.json')).read_text(encoding='utf8'))['layout']
  boxes=markers(slug,data);assert len(boxes)==len(content)-1,(lang,slug)
  highlights=[]
  for n,box in enumerate(boxes,1):
   assert box and box['width']>0 and box['height']>0,(lang,slug)
   x=max(2,box['x']-5);y=max(2,box['y']-5)
   bw=min(box['width']+10,w-x-2);bh=min(box['height']+10,h-y-2)
   assert y<h and bh>0,(lang,slug,box)
   highlights.append(f'<div class="mark" style="left:{x/w*100}%;top:{y/h*100}%;width:{bw/w*100}%;height:{bh/h*100}%"><b>{n}</b></div>')
  notes=''.join(f'<section class="note"><b>{n}</b><h2>{html.escape(heading)}</h2><p>{html.escape(text)}</p></section>' for n,(heading,text) in enumerate(content[1:],1))
  source=f'../../raw/{lang}/{scene}.jpg'
  title=html.escape(content[0])
  page=f'<!doctype html><html lang="{lang if lang!="zh" else "zh-CN"}"><meta charset="utf-8"><link rel="stylesheet" href="../style.css"><title>{title}</title><body class="card"><header><div><small>CoA Server Manager</small><h1>{title}</h1></div><span>{labels[lang]} · {index:02d} / 10</span></header><div class="layout"><div class="shot"><img src="{source}" width="{w}" height="{h}" style="height:{1098*h/w}px;object-fit:cover;object-position:top">{"".join(highlights)}</div><aside>{notes}</aside></div><footer><span>CoA Server Manager · Windows</span><span>github.com/Corfirean/coa-server-manager</span></footer></body></html>'
  (folder/(slug+'.html')).write_text(page,encoding='utf8')
  manifest.append(dict(locale=lang,slug=slug,title=content[0],page=f'{lang}/{slug}.html',width=1500))
(root/'gallery/manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding='utf8')
index='<!doctype html><html lang="ru"><meta charset="utf-8"><title>CoA — переведённые скриншоты</title><style>body{background:#101216;color:#eee;font:17px "Segoe UI",sans-serif;max-width:1200px;margin:40px auto}h1{color:#dfb65d}nav{display:flex;gap:20px;margin:20px 0}a{color:#dfb65d}section{display:grid;grid-template-columns:1fr 1fr;gap:24px}article{background:#191c21;padding:16px;border-radius:12px}img{width:100%;height:auto;display:block}h2{margin-top:50px}p{color:#adb1bc}</style><h1>CoA Server Manager</h1><p>Скриншоты интерфейса и пояснения на пяти языках.</p><nav>'+''.join(f'<a href="#{lang}">{label}</a>' for lang,label in labels.items())+'</nav>'
for lang,label in labels.items():
 index+=f'<h2 id="{lang}">{label}</h2><section>'
 for item in [m for m in manifest if m['locale']==lang]:
  index+=f'<article><a href="{item["page"]}"><h3>{html.escape(item["title"])}</h3><img loading="lazy" src="../exports/{lang}/{item["slug"]}.jpg"></a></article>'
 index+='</section>'
(root/'gallery/index.html').write_text(index,encoding='utf8')
print('Generated',len(manifest),'localized gallery cards')
