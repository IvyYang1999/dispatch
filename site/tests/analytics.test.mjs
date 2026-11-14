import test from 'node:test';
import assert from 'node:assert/strict';
import { installAnalytics, downloadEvent } from '../src/analytics.mjs';

function fixture({host='dispatch.yytyyf.com',dnt=null,gpc=false,consent=null}={}) {
  const listeners={}, scripts=[], writes=[];
  const w={location:{hostname:host,protocol:'https:',pathname:'/',origin:'https://'+host,href:'https://'+host+'/?email=private#secret'},
    navigator:{doNotTrack:dnt,globalPrivacyControl:gpc},localStorage:{getItem:()=>consent,setItem:(...x)=>writes.push(x)},
    addEventListener:(n,f)=>{listeners['window:'+n]=f}};
  const d={documentElement:{lang:'en'},referrer:'https://example.org/private?q=secret',
    createElement:()=>({}),head:{append:s=>scripts.push(s)},addEventListener:(n,f)=>{listeners[n]=f}};
  w.top=w; w.self=w;
  return {w,d,scripts,writes,listeners,start:()=>installAnalytics(w,d,'G-TEST123'),events:()=>Array.from(w.dataLayer??[],x=>Array.from(x))};
}

test('denied consent precedes loading; one page view; sanitized data; no consent writes',()=>{
  const f=fixture(); f.start(); f.start();
  assert.equal(f.scripts.length,1); assert.equal(f.events().filter(e=>e[0]==='event'&&e[1]==='page_view').length,1);
  assert.deepEqual(f.events()[0],['consent','default',{analytics_storage:'denied',ad_storage:'denied',ad_user_data:'denied',ad_personalization:'denied'}]);
  assert.equal(f.writes.length,0);
  const serialized=JSON.stringify(f.events());
  assert.ok(!serialized.includes('granted')); assert.ok(!serialized.includes('private')); assert.ok(!serialized.includes('secret'));
});
test('DNT, GPC, old refusal, preview and iframe never load GA',()=>{
  for(const args of [{dnt:'1'},{gpc:true},{consent:JSON.stringify({value:'denied',at:0})},{host:'localhost'},{host:'dispatch-preview.vercel.app'}]){
    const f=fixture(args);f.start();assert.equal(f.scripts.length,0);assert.equal(f.events().length,0);
  }
  const f=fixture();f.w.top={};f.start();assert.equal(f.scripts.length,0);
});
test('download classification uses exact repository, artifact and version; fallback separate',()=>{
  assert.equal(downloadEvent('https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.18-macos-arm64-signed.zip').name,'file_download');
  assert.equal(downloadEvent('https://github.com/IvyYang1999/dispatch/releases/latest').name,'release_page_open');
  for(const href of ['https://evil.test/a.zip','https://github.com/other/repo/releases/a.zip','https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.17-macos-arm64-signed.zip']) assert.equal(downloadEvent(href),null);
});
test('real link handler records one event without blocking default navigation',()=>{
  const f=fixture();f.start();
  let prevented=false;
  const link={href:'https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.18-macos-arm64-signed.zip?private=secret',dataset:{downloadPlacement:'hero'}};
  const event={type:'click',button:0,target:{closest:()=>link},preventDefault:()=>{prevented=true}};
  f.listeners.click(event);
  assert.equal(prevented,false);
  const sent=f.events().filter(x=>x[0]==='event'&&x[1]==='file_download');
  assert.equal(sent.length,1); assert.equal(sent[0][2].placement,'hero');
  assert.ok(!JSON.stringify(sent).includes('secret'));
  link.href='https://github.com/IvyYang1999/dispatch/releases/latest';
  f.listeners.click(event);
  assert.equal(f.events().filter(x=>x[1]==='file_download').length,1);
  assert.equal(f.events().filter(x=>x[1]==='release_page_open').length,1);
});
test('legacy refusal after startup disables further collection and late script errors do not retry',()=>{
  const f=fixture();f.start();const before=f.events().length;
  f.w.localStorage.getItem=()=>JSON.stringify({value:'denied'});
  f.listeners['window:storage']({key:'dc-analytics-consent-v1'});
  assert.equal(f.w['ga-disable-G-TEST123'],true);
  f.listeners.click({target:{closest:()=>({href:'https://github.com/IvyYang1999/dispatch/releases/latest',dataset:{downloadPlacement:'hero'}})}});
  f.scripts[0].onerror();f.start();
  assert.equal(f.events().length,before);assert.equal(f.scripts.length,1);
});
test('unavailable preferences fail closed and queue remains bounded while tag never loads',()=>{
  const blocked=fixture();blocked.w.localStorage.getItem=()=>{throw Error('unavailable')};blocked.start();assert.equal(blocked.scripts.length,0);
  const f=fixture();f.start();
  for(let n=0;n<300;n++)f.listeners.click({target:{closest:()=>({href:'https://github.com/IvyYang1999/dispatch/releases/latest',dataset:{downloadPlacement:'hero'}})}});
  assert.ok(f.events().length<=100);assert.equal(f.scripts.length,1);
});
