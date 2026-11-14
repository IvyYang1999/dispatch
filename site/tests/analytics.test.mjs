import test from 'node:test';
import assert from 'node:assert/strict';
import { installAnalytics, downloadEvent } from '../src/analytics.mjs';

function fixture({host='dispatch.yytyyf.com',dnt=null,gpc=false,consent=null,referrer='https://example.org/'}={}) {
  const listeners={}, sent=[], injections=[], writes=[];
  const w={location:{hostname:host,protocol:'https:',href:'https://'+host+'/?email=private#secret'},
    navigator:{doNotTrack:dnt,globalPrivacyControl:gpc},localStorage:{getItem:()=>consent,setItem:(...x)=>writes.push(x)}};
  const d={referrer,addEventListener:(n,f)=>{listeners[n]=f}};
  w.top=w;w.self=w;
  let beforeSend;
  const sdk={inject(options){injections.push(options);beforeSend=options.beforeSend;},track(name,data){
    const event=beforeSend({type:'event',url:w.location.href});if(event)sent.push({...event,name,data});
  }};
  const f={w,d,listeners,sent,injections,writes,start:()=>installAnalytics(w,d,sdk),
    view:()=>beforeSend?.({type:'pageview',url:w.location.href}),
    click(href='https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.18-macos-arm64-signed.zip',placement='hero',type='click',button=0){
      let prevented=false;listeners[type]?.({type,button,target:{closest:()=>({href,dataset:{downloadPlacement:placement}})},preventDefault:()=>{prevented=true}});return prevented;
    }};
  return f;
}

test('single injection and page view; only a sanitized URL; no storage writes or Google tag',()=>{
  const f=fixture();f.start();f.start();
  assert.equal(f.injections.length,1);assert.deepEqual(f.view(),{type:'pageview',url:'https://dispatch.yytyyf.com/'});assert.equal(f.view(),null);
  assert.equal(f.writes.length,0);assert.equal(f.w.dataLayer,undefined);
});
test('privacy signals, refusal, preview and iframe prevent SDK loading',()=>{
  for(const args of [{dnt:'1'},{gpc:true},{consent:JSON.stringify({value:'denied'})},{host:'localhost'},{host:'dispatch-preview.vercel.app'}]){
    const f=fixture(args);f.start();assert.equal(f.injections.length,0);
  }
  const f=fixture();f.w.top={};f.start();assert.equal(f.injections.length,0);
  const unreadable=fixture();unreadable.w.localStorage.getItem=()=>{throw Error('unavailable')};unreadable.start();assert.equal(unreadable.injections.length,0);
});
test('detailed incoming referrers are dropped, since SDK cannot redact their contents',()=>{
  for(const referrer of ['https://example.org/private','https://example.org/?email=secret','https://example.org/#token','invalid']){
    const f=fixture({referrer});f.start();assert.equal(f.injections.length,0);
  }
  for(const referrer of ['', 'https://example.org/']){const f=fixture({referrer});f.start();assert.equal(f.injections.length,1);}
});
test('exact artifact/version/repository classification keeps release-page clicks separate',()=>{
  assert.equal(downloadEvent('https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.18-macos-arm64-signed.zip').name,'download_click');
  assert.equal(downloadEvent('https://github.com/IvyYang1999/dispatch/releases/latest').name,'release_page_open');
  for(const href of ['https://evil.test/a.zip','https://github.com/other/repo/releases/a.zip','https://github.com/IvyYang1999/dispatch/releases/download/v0.1.18/Dispatch-0.1.17-macos-arm64-signed.zip'])assert.equal(downloadEvent(href),null);
});
test('clicks preserve navigation, send two safe properties, and distinguish middle/right clicks',()=>{
  const f=fixture();f.start();assert.equal(f.click(),false);assert.equal(f.sent.length,1);
  assert.deepEqual(f.sent[0].data,{target:'macos_arm64',placement:'hero'});
  f.click('https://github.com/IvyYang1999/dispatch/releases/latest?private=secret','other_platforms','auxclick',1);
  assert.equal(f.sent[1].name,'release_page_open');assert.ok(!JSON.stringify(f.sent).includes('secret'));
  f.click(undefined,'hero','auxclick',2);f.click(undefined,'unknown');assert.equal(f.sent.length,2);
});
test('changed refusal and GPC stop pending and future events; per-page attempts are bounded',()=>{
  const f=fixture();f.start();f.w.localStorage.getItem=()=>JSON.stringify({value:'denied'});assert.equal(f.view(),null);f.click();assert.equal(f.sent.length,0);
  const g=fixture();g.start();g.w.navigator.globalPrivacyControl=true;g.click();assert.equal(g.view(),null);assert.equal(g.sent.length,0);
  const h=fixture();h.start();for(let n=0;n<300;n++)h.click();assert.equal(h.sent.length,99);assert.equal(h.injections.length,1);
});
test('SDK failures never break downloads and do not cause reinjection retries',()=>{
  const f=fixture();const sdk={inject(){throw Error('blocked')},track(){throw Error('blocked')}};
  assert.doesNotThrow(()=>installAnalytics(f.w,f.d,sdk));assert.doesNotThrow(()=>installAnalytics(f.w,f.d,sdk));
  assert.equal(f.w.__dispatchAnalytics.enabled,false);
});
