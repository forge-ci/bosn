// Execute the real compositor script against monitor/dock geometry.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const script = fs.readFileSync('crates/bosn-service/src/bin/bosn/widget/placement.js', 'utf8');
let added;
let timer;
const calls=[];
const warnings=[];
function QTimer() { timer=this; this.timeout=signal(); this.start=()=>{}; this.stop=()=>{this.stopped=true;}; }
let markerPresent = false;
function callDBus(...args) {
    calls.push(args);
    const callback = args.at(-1);
    let result;
    if (args[3] === 'isScriptLoaded') result = markerPresent;
    else if (args[3] === 'unloadScript') { result = markerPresent; markerPresent = false; }
    else { result = markerPresent ? -1 : 3; markerPresent = true; }
    if (typeof callback === 'function') callback(result);
}
function signal() { const callbacks=[]; return { connect:f=>callbacks.push(f), emit:()=>callbacks.forEach(f=>f()) }; }
const screensChanged=signal(), virtualScreenGeometryChanged=signal(), removed=signal();
const dockChanged=signal();
// Some Plasma versions omit dock visibility signals; geometry still works.
const dock={dock:true, frameGeometryChanged:dockChanged};
const bubble = { caption: 'bosn bubble', resourceClass: 'dev.bosn.widget', frameGeometry: {x:2667,y:761,width:72,height:72}, active:false, outputChanged:signal(), windowShown:signal() };
const panel = { caption:'bosn panel', resourceClass:'dev.bosn.widget', frameGeometry:{x:20,y:30,width:420,height:640}, active:false, outputChanged:signal(), windowShown:signal() };
const other = { caption:'bosn bubble', resourceClass:'other', frameGeometry:{x:7,y:8,width:72,height:72} };
let area = {x:1423,y:99,width:2560,height:1396};
vm.runInNewContext(script, { QTimer,callDBus,console:{warn:message=>warnings.push(message)},KWin:{MaximizeArea:1}, workspace:{windowList:()=>[bubble,panel,other,dock], clientArea:()=>area, windowAdded:{connect:f=>added=f},screensChanged,virtualScreenGeometryChanged,windowRemoved:removed} });
assert.equal(JSON.stringify(bubble.frameGeometry), JSON.stringify({x:3887,y:1399,width:72,height:72}));
assert.equal(bubble.active,false);
assert.equal(bubble.keepAbove,true);
assert.equal(panel.frameGeometry.x,3539);
assert.equal(panel.frameGeometry.y,831);
assert.equal(panel.active,false);
assert.equal(other.frameGeometry.x,7);
area = {x:-1600,y:0,width:1600,height:860};
const next = {...bubble,frameGeometry:{x:0,y:0,width:72,height:72}};
added(next);
assert.equal(next.frameGeometry.x,-96);
assert.equal(next.frameGeometry.y,764);
area={x:1423,y:99,width:2560,height:1300};
dockChanged.emit();
assert.equal(bubble.frameGeometry.y,1303);
area={x:1423,y:99,width:2560,height:1396};
removed.emit();
assert.equal(bubble.frameGeometry.y,1399);
area={x:0,y:0,width:1920,height:1080};
screensChanged.emit();
assert.equal(bubble.frameGeometry.x,1824);
assert.equal(bubble.frameGeometry.y,984);
area={x:0,y:0,width:1280,height:720};
virtualScreenGeometryChanged.emit();
assert.equal(bubble.frameGeometry.x,1184);
area={x:-1600,y:0,width:1600,height:860};
bubble.outputChanged.emit();
assert.equal(bubble.frameGeometry.x,-96);
console.log('widget placement: dock clearance, negative monitor origin, autoload, focus and window scope passed');

timer.timeout.emit();
assert.equal(calls.at(-1)[3], 'loadScript');
timer.timeout.emit();
assert.equal(calls.at(-1)[3], 'unloadScript');
assert.deepEqual(warnings, []);
// A compositor that rejects geometry withdraws readiness without stopping the
// timer (#692): one tick of grace for asynchronous placement, then one
// diagnostic, bounded backoff, and recovery once placement is applied again.
let rejected = bubble.frameGeometry;
let accept = false;
let stored = rejected;
Object.defineProperty(bubble, 'frameGeometry', {get:()=>accept ? stored : rejected, set:value=>{stored=value;}});
area={x:0,y:0,width:800,height:600};
const before = calls.length;
timer.timeout.emit();
assert.equal(calls.length, before, 'a first mismatch waits one tick for asynchronous placement');
assert.deepEqual(warnings, []);
timer.timeout.emit();
assert.notEqual(timer.stopped,true,'a rejected readback must not stop readiness for the session');
assert.equal(calls.at(-1)[3],'unloadScript');
assert.equal(warnings.length,1);
assert.match(warnings[0], /geometry readback rejected.*bosn widget install/);
for (let tick = 0; tick < 400; tick++) timer.timeout.emit();
assert.equal(warnings.length,1,'a persisting rejection is reported once per transition');
const rejectedCalls = calls.length - before;
assert.ok(rejectedCalls <= 12, `backoff bounds compositor calls while rejected (${rejectedCalls})`);
accept = true;
for (let tick = 0; tick < 300 && warnings.length < 2; tick++) timer.timeout.emit();
assert.equal(warnings.length,2,'recovery is reported once');
assert.match(warnings[1], /recovered/);
const marks = calls.length;
timer.timeout.emit();
timer.timeout.emit();
assert.ok(calls.length > marks, 'readiness pulses resume after recovery');
console.log('widget placement: rejected readback backs off and recovers');
