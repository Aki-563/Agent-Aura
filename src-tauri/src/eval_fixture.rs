//! A tiny local web server with test pages for the eval suite. Each page reports what the
//! agent did (clicks, form values, scrolling, drags) back to this process, so a task can be
//! checked independently of what the model claims.
//!
//! Only listens on 127.0.0.1 and serves fixed pages; it never touches the network or any file.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const PORT: u16 = 47831;

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static STARTED: OnceLock<bool> = OnceLock::new();
/// Latest value of each named page state (input text, scroll offsets, selection...).
static STATE: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
/// Latest client-area rectangle (left, top, width, height in CSS pixels) of each lab element.
static RECTS: Mutex<BTreeMap<String, [f64; 4]>> = Mutex::new(BTreeMap::new());
/// Command for the lab page: (sequence number, text). The page acknowledges through the state `ack`.
static COMMAND: Mutex<(u64, String)> = Mutex::new((0, String::new()));

/// Starts the server once per process. Returns false when the port is unavailable.
pub fn start() -> bool {
    *STARTED.get_or_init(|| match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    std::thread::spawn(move || {
                        let _ = handle(stream);
                    });
                }
            });
            true
        }
        Err(_) => false,
    })
}

pub fn clear() {
    if let Ok(mut events) = EVENTS.lock() {
        events.clear();
    }
}

pub fn events() -> Vec<String> {
    EVENTS.lock().map(|events| events.clone()).unwrap_or_default()
}

/// True when an event equal to `wanted` (case-insensitive) was reported.
pub fn saw(wanted: &str) -> bool {
    events().iter().any(|event| event.eq_ignore_ascii_case(wanted))
}

/// Latest value of a page state, or None when the page never reported it.
pub fn state(key: &str) -> Option<String> {
    STATE.lock().ok().and_then(|state| state.get(key).cloned())
}

/// Rectangle (left, top, width, height) of a lab element in the page's CSS pixels.
pub fn rect(id: &str) -> Option<[f64; 4]> {
    RECTS.lock().ok().and_then(|rects| rects.get(id).copied())
}

/// Forgets one state value (so a test can wait for a fresh report).
pub fn clear_state(key: &str) {
    if let Ok(mut state) = STATE.lock() {
        state.remove(key);
    }
}

pub fn clear_rects() {
    if let Ok(mut rects) = RECTS.lock() {
        rects.clear();
    }
    if let Ok(mut state) = STATE.lock() {
        state.clear();
    }
}

/// Queues a command for the lab page and returns its sequence number.
pub fn send_command(text: &str) -> u64 {
    let mut command = COMMAND.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    command.0 += 1;
    command.1 = text.to_owned();
    command.0
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(value) = hex {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > 64 * 1024 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let request_line = head.lines().next().unwrap_or_default().to_owned();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or("/").split('?').next().unwrap_or("/").to_owned();
    let length = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[header_end..]).to_string();

    let (status, content_type, payload) = if method == "POST" && path == "/event" {
        if let Ok(mut events) = EVENTS.lock() {
            events.push(body.trim().to_owned());
        }
        ("200 OK", "text/plain", String::new())
    } else if method == "POST" && path == "/state" {
        if let Some((key, value)) = body.split_once('=') {
            if let Ok(mut state) = STATE.lock() {
                state.insert(key.to_owned(), percent_decode(value));
            }
        }
        ("200 OK", "text/plain", String::new())
    } else if method == "POST" && path == "/rects" {
        if let Ok(mut rects) = RECTS.lock() {
            for line in body.lines() {
                if let Some((id, values)) = line.split_once('=') {
                    let numbers: Vec<f64> = values.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                    if numbers.len() >= 4 {
                        rects.insert(id.to_owned(), [numbers[0], numbers[1], numbers[2], numbers[3]]);
                    }
                }
            }
        }
        ("200 OK", "text/plain", String::new())
    } else if path == "/cmd" {
        let command = COMMAND.lock().map(|c| format!("{}|{}", c.0, c.1)).unwrap_or_default();
        ("200 OK", "text/plain", command)
    } else if path == "/events" {
        ("200 OK", "text/plain", events().join("\n"))
    } else {
        match page(&path) {
            Some(html) => ("200 OK", "text/html; charset=utf-8", html),
            None => ("404 Not Found", "text/plain", "not found".to_owned()),
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(response.as_bytes())
}

const STYLE: &str = "<style>body{font-family:Segoe UI,Arial,sans-serif;margin:40px;font-size:20px}\
button{font-size:20px;padding:12px 26px;margin:10px;cursor:pointer}input,select{font-size:20px;padding:8px;margin:8px}\
.box{display:inline-block;width:240px;height:120px;border:3px solid #333;margin:20px;text-align:center;line-height:120px;user-select:none}</style>";

fn shell(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><title>Aura Fixture {title}</title>{STYLE}\
<script>function ev(s){{fetch('/event',{{method:'POST',body:s}})}}window.addEventListener('load',function(){{ev('load:'+document.title)}})</script></head><body>{body}</body></html>"
    )
}


/// Situations a page throws at the harness: browser dialogs, a link that opens a new tab, a menu that
/// only exists while the pointer is over it, content that arrives late, and a disabled button.
const TOUGH_BODY: &str = r##"
<style>#menu{display:inline-block;padding:14px 30px;border:2px solid #333;margin:10px;background:#eee}</style>
<h1>Tough situations</h1>
<div><button onclick="alert('Aura alert');ev('alert:closed')">Show alert</button>
<button onclick="ev('confirm:'+confirm('Proceed with the order?'))">Ask confirm</button></div>
<div><a href="/links/about" target="_blank">Open in new tab</a></div>
<div id=menu onmouseenter="document.getElementById('sub').style.display='block'" onmouseleave="document.getElementById('sub').style.display='none'">Products
<div id=sub style="display:none"><button onclick="ev('menu:pricing')">Pricing menu item</button></div></div>
<div><button onclick="setTimeout(function(){document.getElementById('slow').innerHTML='<button onclick=&quot;ev(\'click:report\')&quot;>Report ready</button>'},2500)">Load report</button><span id=slow></span></div>
<div><button disabled onclick="ev('click:disabled')">Disabled action</button><button onclick="ev('click:enabled')">Enabled action</button></div>
"##;

/// The "lab": one page with every kind of control, used by the conformance suite to check that
/// each input the harness can send arrives exactly as ordered.
const LAB_BODY: &str = r##"
<style>.row{margin:6px 0}#inner{width:320px;height:130px;overflow:auto;border:2px solid #555}
#wide{width:320px;overflow-x:auto;border:2px solid #555}#wide div{width:2400px;height:40px;background:linear-gradient(90deg,#fdd,#ddf)}
#track{position:relative;width:320px;height:26px;background:#ccc;margin:8px 0}
#thumb{position:absolute;left:0;top:0;width:26px;height:26px;background:#36c;cursor:grab}
#src{position:relative;display:inline-block;width:120px;height:50px;background:#fa3;line-height:50px;text-align:center;user-select:none}
#dst{display:inline-block;width:200px;height:90px;border:3px dashed #333;line-height:90px;text-align:center;margin-left:60px}
#hsrc{display:inline-block;width:150px;height:46px;background:#9d9;line-height:46px;text-align:center}
#hdst{display:inline-block;width:200px;height:70px;border:3px dashed #070;line-height:70px;text-align:center;margin-left:60px}
.box{width:200px;height:70px;line-height:70px;font-size:18px}</style>
<h1 id=title>Lab</h1>
<div class=row><button id=btn-a>A</button><button id=btn-b>B</button><button id=btn-c>C</button>
<button id=tiny style="font-size:10px;padding:0;width:22px;height:22px;margin-left:30px">s</button>
<span id=px1 style="display:inline-block;width:1px;height:1px;background:#f00;margin-left:40px;vertical-align:middle"></span></div>
<div class=row><div id=box class=box style="display:inline-block;border:3px solid #333;text-align:center;user-select:none">click box</div>
<div id=hov class=box style="display:inline-block;border:3px solid #a33;text-align:center;margin-left:30px">hover me</div></div>
<form id=f class=row><input id=i1 type=text placeholder="field one"><input id=i2 type=text placeholder="field two"><button type=submit>Go</button></form>
<div class=row><textarea id=area rows=3 cols=40></textarea></div>
<p><span id=para style="display:inline-block">The quick brown fox jumps over the lazy dog.</span></p>
<div class=row><div id=inner></div><div style="height:8px"></div><div id=wide><div></div></div></div>
<div class=row><div id=track><div id=thumb></div></div></div>
<div class=row><div id=src>drag me</div><div id=dst>drop here</div></div>
<div class=row><div id=hsrc draggable=true>html5 drag</div><div id=hdst>html5 drop</div></div>
<div style="height:2200px;background:linear-gradient(#fff,#eef)">spacer</div>
<button id=bottom>Bottom</button><div style="height:60px"></div>
<script>
var $=function(i){return document.getElementById(i)};
function st(k,v){fetch('/state',{method:'POST',body:k+'='+encodeURIComponent(v)})}
function mods(e){return (e.ctrlKey?'ctrl+':'')+(e.altKey?'alt+':'')+(e.shiftKey?'shift+':'')}
var inner=$('inner'),wide=$('wide');
for(var n=1;n<=40;n++){var d=document.createElement('div');d.textContent='Inner line '+n;inner.appendChild(d)}
['btn-a','btn-b','btn-c','tiny','px1','bottom'].forEach(function(id){$(id).onclick=function(){ev('click:'+id)}});
var box=$('box');
box.onclick=function(e){ev('click:box:d'+e.detail);if(e.shiftKey)ev('click:box:shift');if(e.ctrlKey)ev('click:box:ctrl')};
box.ondblclick=function(){ev('dblclick:box')};
box.oncontextmenu=function(e){e.preventDefault();ev('contextmenu:box')};
box.onauxclick=function(e){if(e.button==1)ev('auxclick:box:middle')};
box.onmousedown=function(e){if(e.button==1)e.preventDefault()};
$('hov').onmouseenter=function(){ev('hover:enter')};$('hov').onmouseleave=function(){ev('hover:leave')};
document.addEventListener('keydown',function(e){ev('key:'+mods(e)+e.key)});
['i1','i2','area'].forEach(function(id){var el=$(id);
 el.oninput=function(){st(id,el.value)};
 var sel=function(){st('sel',el.value.substring(el.selectionStart,el.selectionEnd))};
 el.onkeyup=sel;el.onmouseup=sel;el.onselect=sel});
$('f').onsubmit=function(e){e.preventDefault();ev('submit:form')};
document.onselectionchange=function(){st('selection',String(window.getSelection()))};
function pageState(){st('scrollY',Math.round(window.scrollY));st('scrollMax',document.documentElement.scrollHeight-window.innerHeight)}
window.onscroll=pageState;
inner.onscroll=function(){st('innerTop',Math.round(inner.scrollTop))};
wide.onscroll=function(){st('innerLeft',Math.round(wide.scrollLeft))};
document.addEventListener('pointermove',function(e){st('move',e.clientX+','+e.clientY)});
var drag=null,thumb=$('thumb'),src=$('src');
thumb.onmousedown=function(e){drag={k:'t',x:e.clientX,l:thumb.offsetLeft};e.preventDefault()};
src.onmousedown=function(e){drag={k:'s',x:e.clientX,y:e.clientY};e.preventDefault()};
document.addEventListener('mousemove',function(e){if(!drag)return;
 if(drag.k=='t'){var l=Math.max(0,Math.min(294,drag.l+e.clientX-drag.x));thumb.style.left=l+'px';st('slider',Math.round(l))}
 else{src.style.left=(e.clientX-drag.x)+'px';src.style.top=(e.clientY-drag.y)+'px'}});
document.addEventListener('mouseup',function(e){if(!drag)return;var k=drag.k;drag=null;
 if(k=='s'){var r=$('dst').getBoundingClientRect();
  ev(e.clientX>r.left&&e.clientX<r.right&&e.clientY>r.top&&e.clientY<r.bottom?'drop:ok':'drop:miss')}});
$('hdst').ondragover=function(e){e.preventDefault()};
$('hdst').ondrop=function(e){e.preventDefault();ev('html5drop:ok')};
function report(){var ids=['title','btn-a','btn-b','btn-c','tiny','px1','box','hov','i1','i2','area','para','inner','wide','track','thumb','src','dst','hsrc','hdst','bottom'];
 var lines=ids.map(function(id){var r=$(id).getBoundingClientRect();return id+'='+r.left+','+r.top+','+r.width+','+r.height});
 lines.push('win='+window.innerWidth+','+window.innerHeight+','+window.devicePixelRatio+',0');
 fetch('/rects',{method:'POST',body:lines.join('\n')})}
var seq=0;
function poll(){fetch('/cmd').then(function(r){return r.text()}).then(function(t){
 var p=t.indexOf('|');var n=parseInt(t.substring(0,p));var c=t.substring(p+1);
 if(n>seq){seq=n;run(c);st('ack',n)}})}
function run(c){
 if(c=='reset'){window.scrollTo(0,0);inner.scrollTop=0;wide.scrollLeft=0;
  ['i1','i2','area'].forEach(function(id){$(id).value=''});
  thumb.style.left='0px';src.style.left='0px';src.style.top='0px';drag=null;
  if(document.activeElement&&document.activeElement.blur)document.activeElement.blur();
  window.getSelection().removeAllRanges();
  st('i1','');st('i2','');st('area','');st('sel','');st('slider','0');st('innerTop','0');st('innerLeft','0');pageState()}
 else if(c.indexOf('into:')==0){$(c.substring(5)).scrollIntoView({block:'center'})}}
setInterval(report,250);setInterval(poll,200);pageState();
</script>
"##;

/// A drawing surface (Phase 9, §H0): draws a line between every two received points (like a paint
/// app), records each event's position (including coalesced events), pointer type, pressure and
/// tilt, and reports the whole stroke as state `stroke` when the pointer lifts. Positions are in
/// CSS pixels relative to the canvas. The pointer position outside a stroke is reported as `move`
/// so the tests can map page pixels to screen pixels exactly.
const CANVAS_BODY: &str = r##"<style>body{margin:8px;overflow:hidden}#cv{border:1px solid #333;touch-action:none;display:block;cursor:crosshair}</style>
<h1 id=title style="margin:2px 0;font-size:14px">Canvas</h1>
<canvas id=cv width=900 height=560></canvas>
<script>
function st(k,v){fetch('/state',{method:'POST',body:k+'='+encodeURIComponent(v)})}
var cv=document.getElementById('cv'),g=cv.getContext('2d'),pts=[],down=false,info={};
function report(){var r=cv.getBoundingClientRect();fetch('/rects',{method:'POST',body:'cv='+r.left+','+r.top+','+r.width+','+r.height+'\nwin='+innerWidth+','+innerHeight+','+devicePixelRatio+',0'})}
setInterval(report,250);report();
function add(e){var r=cv.getBoundingClientRect();var list=(e.getCoalescedEvents&&e.getCoalescedEvents().length)?e.getCoalescedEvents():[e];
 list.forEach(function(c){var p=[+(c.clientX-r.left).toFixed(2),+(c.clientY-r.top).toFixed(2)];
  if(pts.length){var a=pts[pts.length-1];g.beginPath();g.moveTo(a[0],a[1]);g.lineTo(p[0],p[1]);g.lineWidth=2;g.stroke()}
  pts.push(p);info.events++;info.maxPressure=Math.max(info.maxPressure,c.pressure||0);info.maxTilt=Math.max(info.maxTilt,Math.abs(c.tiltX||0),Math.abs(c.tiltY||0))})}
cv.addEventListener('pointerdown',function(e){down=true;pts=[];info={type:e.pointerType,events:0,maxPressure:0,maxTilt:0,buttons:e.buttons};try{cv.setPointerCapture(e.pointerId)}catch(x){}add(e)});
document.addEventListener('pointermove',function(e){if(down){add(e)}else{st('move',e.clientX+','+e.clientY)}});
cv.addEventListener('pointerup',function(e){if(!down)return;down=false;add(e);st('stroke',JSON.stringify({info:info,pts:pts}));ev('stroke:'+info.type)});
var seq=0;setInterval(function(){fetch('/cmd').then(function(r){return r.text()}).then(function(t){var p=t.indexOf('|');var n=parseInt(t.substring(0,p));if(n>seq){seq=n;if(t.substring(p+1)=='clear'){g.clearRect(0,0,cv.width,cv.height)}st('ack',n)}})},200);
</script>"##;

/// Slow-loading page (Phase 9, §H0/§H4): `/load?delay=<ms>&kind=spinner|progress|skeleton|infinite`.
/// "Load data" shows the loading UI, then the content after `delay` ms (never for `infinite`).
/// The page reports `ready` = the time the content appeared, so a test can tell whether the
/// harness's wait ended before the content was there (an early action).
const LOAD_BODY: &str = r##"<style>
#spin{width:40px;height:40px;border:6px solid #ccc;border-top-color:#36c;border-radius:50%;animation:r .8s linear infinite;display:none;margin:20px}
@keyframes r{to{transform:rotate(360deg)}}
#bar{width:400px;height:14px;background:#eee;display:none;margin:20px}#fill{height:100%;width:0;background:#36c}
.sk{width:420px;height:18px;margin:8px 20px;background:linear-gradient(90deg,#eee 25%,#ddd 50%,#eee 75%);background-size:200% 100%;animation:sh 1s linear infinite;display:none}
@keyframes sh{to{background-position:-200% 0}}
#content{display:none;font-size:22px;margin:20px}
</style>
<h1 id=title>Load test</h1>
<button id=go style="font-size:18px">Load data</button>
<div id=spin></div><div id=bar><div id=fill></div></div><div class=sk></div><div class=sk></div><div class=sk></div>
<div id=content><button id=done onclick="ev('click:done')">Data ready</button></div>
<script>
function st(k,v){fetch('/state',{method:'POST',body:k+'='+encodeURIComponent(v)})}
var q=new URLSearchParams(location.search),delay=+(q.get('delay')||1000),kind=q.get('kind')||'spinner';
document.getElementById('go').onclick=function(){var t0=performance.now();st('ready','');ev('loading');
 if(kind=='spinner'||kind=='infinite')document.getElementById('spin').style.display='block';
 if(kind=='progress'){document.getElementById('bar').style.display='block';var f=document.getElementById('fill');var iv=setInterval(function(){var p=Math.min(100,(performance.now()-t0)/delay*100);f.style.width=p+'%';if(p>=100)clearInterval(iv)},50)}
 if(kind=='skeleton')document.querySelectorAll('.sk').forEach(function(e){e.style.display='block'});
 if(kind=='infinite')return;
 setTimeout(function(){document.getElementById('spin').style.display='none';document.getElementById('bar').style.display='none';
  document.querySelectorAll('.sk').forEach(function(e){e.style.display='none'});document.getElementById('content').style.display='block';
  st('ready',String(Date.now()));ev('ready')},delay)};
</script>"##;

fn page(path: &str) -> Option<String> {
    Some(match path {
        "/click" => shell(
            "Click",
            "<h1>Click test</h1><p>Press the button named Beta.</p>\
<button id=alpha onclick=\"ev('click:alpha')\">Alpha</button>\
<button id=beta onclick=\"ev('click:beta')\">Beta</button>\
<button id=gamma onclick=\"ev('click:gamma')\">Gamma</button>",
        ),
        "/form" => shell(
            "Form",
            "<h1>Sign-up form</h1><form onsubmit=\"event.preventDefault();ev('form:name='+n.value+';color='+c.value+';agree='+a.checked)\">\
<label>Name <input id=n type=text></label><br>\
<label>Favorite color <select id=c><option value=red>Red</option><option value=green>Green</option><option value=blue>Blue</option></select></label><br>\
<label><input id=a type=checkbox> I agree to the terms</label><br>\
<button type=submit>Submit form</button></form>",
        ),
        "/scroll" => {
            let filler: String = (1..=45).map(|i| format!("<p>Paragraph {i}: scroll further down, the button is at the very bottom.</p>")).collect();
            shell("Scroll", &format!("<h1>Scroll test</h1>{filler}<button onclick=\"ev('click:finish')\">Finish</button><p>End of page.</p>"))
        }
        "/mouse" => shell(
            "Mouse",
            "<h1>Mouse test</h1><p>Double-click the left box, then right-click the right box.</p>\
<div class=box ondblclick=\"ev('dblclick:left');this.style.background='#9f9'\">Double-click me</div>\
<div class=box oncontextmenu=\"event.preventDefault();ev('rightclick:right');this.style.background='#9cf'\">Right-click me</div>",
        ),
        "/drag" => shell(
            "Drag",
            "<h1>Drag test</h1><p>Drag the orange square into the dashed box.</p>\
<div id=src class=box style='background:#fa3;width:120px;height:60px;line-height:60px;position:relative;cursor:grab'>Drag me</div>\
<div id=dst class=box style='border-style:dashed'>Drop here</div>\
<script>var d=null,ox=0,oy=0,s=document.getElementById('src');\
s.onmousedown=function(e){d=1;ox=e.clientX;oy=e.clientY;e.preventDefault()};\
document.onmousemove=function(e){if(d){s.style.left=(e.clientX-ox)+'px';s.style.top=(e.clientY-oy)+'px'}};\
document.onmouseup=function(e){if(!d)return;d=null;var r=document.getElementById('dst').getBoundingClientRect();\
if(e.clientX>r.left&&e.clientX<r.right&&e.clientY>r.top&&e.clientY<r.bottom)ev('drop:ok');else ev('drop:miss')};</script>",
        ),
        "/cookies" => shell(
            "Cookies",
            "<h1>Newsletter</h1><p>Continue becomes available after accepting the cookie notice.</p>\
<button id=cont disabled onclick=\"ev('click:continue')\">Continue</button>\
<div id=ban style='position:fixed;left:0;right:0;bottom:0;background:#222;color:#fff;padding:24px;text-align:center'>We use cookies. \
<button onclick=\"ev('click:accept');document.getElementById('ban').style.display='none';document.getElementById('cont').disabled=false\">Accept</button></div>",
        ),
        "/links" => shell(
            "Links",
            "<h1>Links</h1><p>Open the page about pricing.</p>\
<a href='/links/about'>About us</a> | <a href='/links/pricing'>Pricing</a> | <a href='/links/contact'>Contact</a>",
        ),
        "/lab" => shell("Lab", LAB_BODY),
        "/tough" => shell("Tough", TOUGH_BODY),
        "/shift" => shell(
            "Shift",
            "<h1>Layout shift</h1><div id=spacer style='height:0'></div><button onclick=\"ev('click:target')\">Target</button><script>document.addEventListener('keydown',function(e){if(e.key=='F9'){document.getElementById('spacer').style.height='160px';ev('shifted')}})</script>",
        ),
        "/canvas" => shell("Canvas", CANVAS_BODY),
        "/login" => shell("Login", "<h1>Sign in</h1><p><input id=user type=text placeholder=\"Username\"></p><p><input id=pw type=password placeholder=\"Password\"></p><p><label><input type=checkbox> I'm not a robot</label></p>"),
        "/load" => shell("Load", LOAD_BODY),
        "/links/pricing" => shell("Pricing", "<h1>Pricing</h1><script>ev('visit:pricing')</script><p>The plan costs 5 dollars.</p>"),
        "/links/about" => shell("About", "<h1>About us</h1><script>ev('visit:about')</script>"),
        "/links/contact" => shell("Contact", "<h1>Contact</h1><script>ev('visit:contact')</script>"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", PORT)).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn serves_pages_and_records_events() {
        assert!(start());
        clear();
        assert!(get("/click").contains("Beta"));
        assert!(get("/nope").starts_with("HTTP/1.1 404"));
        let mut stream = TcpStream::connect(("127.0.0.1", PORT)).unwrap();
        write!(stream, "POST /event HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nclick:beta").unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        assert!(saw("click:beta"));
        assert!(!saw("click:alpha"));
        clear();
        assert!(events().is_empty());
    }
}
