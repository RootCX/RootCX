(()=>{
const I=new Set(['link','button','textbox','searchbox','combobox','listbox','checkbox','radio','switch','slider','spinbutton','tab','menuitem','menuitemcheckbox','menuitemradio','option','treeitem','textarea']);
const L=new Set(['heading','main','table','navigation','banner']);
const T={A:'link',BUTTON:'button',INPUT:'textbox',TEXTAREA:'textarea',SELECT:'combobox',H1:'heading',H2:'heading',H3:'heading',H4:'heading',H5:'heading',H6:'heading',NAV:'navigation',MAIN:'main',TABLE:'table'};
const R=[];let n=0,chars=0;
function role(e){return e.getAttribute('role')||T[e.tagName]||''}
function name(e){return e.getAttribute('aria-label')||e.getAttribute('placeholder')||e.getAttribute('title')||e.getAttribute('alt')||(e.innerText||'').trim().slice(0,200)}
function sel(e){if(e.id)return'#'+CSS.escape(e.id);const p=[];let c=e;while(c&&c!==document.body&&p.length<5){let s=c.tagName.toLowerCase();const pr=c.parentElement;if(pr){const sb=[...pr.children].filter(x=>x.tagName===c.tagName);if(sb.length>1)s+=':nth-child('+([...pr.children].indexOf(c)+1)+')'}p.unshift(s);c=pr}return p.join('>')}
function walk(e){if(chars>=20000)return;const s=getComputedStyle(e);if(s.display==='none'||s.visibility==='hidden'||s.opacity==='0')return;if(e.getAttribute('aria-hidden')==='true')return;const r=e.getBoundingClientRect();if(!r.width||!r.height)return;const ro=role(e),na=name(e),cur=s.cursor==='pointer',int=I.has(ro)||cur,lm=L.has(ro)&&na;if((int||lm)&&na){const k=int?'i':'l';R.push({idx:n++,kind:k,role:ro||'element',name:na.slice(0,200),sel:sel(e)});chars+=ro.length+na.length+20}for(const c of e.children)walk(c)}
walk(document.body||document.documentElement);return JSON.stringify(R)})()
