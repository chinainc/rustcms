!(function(){"use strict";var W=(O,T,a)=>new Promise((c,h)=>{var D=s=>{try{l(a.next(s))}catch(y){h(y)}},C=s=>{try{l(a.throw(s))}catch(y){h(y)}},l=s=>s.done?c(s.value):Promise.resolve(s.value).then(D,C);l((a=a.apply(O,T)).next())});(self.webpackChunkRustCMS=self.webpackChunkRustCMS||[]).push([[3755],{10646:(function(O,T,a){a.d(T,{H:function(){return h}});var c=a(82933);function h(D,C){var l=D.append("foreignObject").attr("width","100000"),s=l.append("xhtml:div");s.attr("xmlns","http://www.w3.org/1999/xhtml");var y=C.label;switch(typeof y){case"function":s.insert(y);break;case"object":s.insert(function(){return y});break;default:s.html(y)}c.AV(s,C.labelStyle),s.style("display","inline-block"),s.style("white-space","nowrap");var L=s.node().getBoundingClientRect();return l.attr("width",L.width).attr("height",L.height),l}}),82933:(function(O,T,a){a.d(T,{AV:function(){return y},De:function(){return D},c$:function(){return $},gh:function(){return C},nh:function(){return L}});var c=a(89610),h=a(34963);function D(d,g){return!!d.children(g).length}function C(d){return s(d.v)+":"+s(d.w)+":"+s(d.name)}var l=/:/g;function s(d){return d?String(d).replace(l,"\\:"):""}function y(d,g){g&&d.attr("style",g)}function L(d,g,v){g&&d.attr("class",g).attr("class",v+" "+d.attr("class"))}function $(d,g){var v=g.graph();if(h.A(v)){var K=v.transition;if(c.A(K))return K(d)}return d}}),75937:(function(O,T,a){var c=a(72453),h=a(74886);const D=(C,l)=>c.A.lang.round(h.A.parse(C)[l]);T.A=D}),13755:(function(O,T,a){var c=a(95939),h=a(49406),D=a(21845),C=a(49853),l=a(697),s=a(43223),y=a(14075),L=a(74353),$=a(16750),d=a(99418);const g={parser:c.p,db:c.f,renderer:h.f,styles:h.a,init:v=>{v.flowchart||(v.flowchart={}),v.flowchart.arrowMarkerAbsolute=v.arrowMarkerAbsolute,(0,D.p)({flowchart:{arrowMarkerAbsolute:v.arrowMarkerAbsolute}}),h.f.setConf(v.flowchart),c.f.clear(),c.f.setGen("gen-2")}};a.d(T,["diagram",0,g])}),49406:(function(O,T,a){var c=a(49853),h=a(10646),D=a(697),C=a(15414),l=a(21845),s=a(25582),y=a(75937);const L={},$=function(t){const _=Object.keys(t);for(const A of _)L[A]=t[A]},d=function(t,_,A,b,f,w){return W(this,null,function*(){const x=b.select(`[id="${A}"]`),n=Object.keys(t);for(const p of n){const r=t[p];let S="default";r.classes.length>0&&(S=r.classes.join(" ")),S=S+" flowchart-label";const E=(0,l.k)(r.styles);let e=r.text!==void 0?r.text:r.id,i;if(l.l.info("vertex",r,r.labelType),r.labelType==="markdown")l.l.info("vertex",r,r.labelType);else if((0,l.m)((0,l.c)().flowchart.htmlLabels)){const M={label:e};i=(0,h.H)(x,M).node(),i.parentNode.removeChild(i)}else{const M=f.createElementNS("http://www.w3.org/2000/svg","text");M.setAttribute("style",E.labelStyle.replace("color:","fill:"));const R=e.split(l.e.lineBreakRegex);for(const N of R){const I=f.createElementNS("http://www.w3.org/2000/svg","tspan");I.setAttributeNS("http://www.w3.org/XML/1998/namespace","xml:space","preserve"),I.setAttribute("dy","1em"),I.setAttribute("x","1"),I.textContent=N,M.appendChild(I)}i=M}let u=0,o="";switch(r.type){case"round":u=5,o="rect";break;case"square":o="rect";break;case"diamond":o="question";break;case"hexagon":o="hexagon";break;case"odd":o="rect_left_inv_arrow";break;case"lean_right":o="lean_right";break;case"lean_left":o="lean_left";break;case"trapezoid":o="trapezoid";break;case"inv_trapezoid":o="inv_trapezoid";break;case"odd_right":o="rect_left_inv_arrow";break;case"circle":o="circle";break;case"ellipse":o="ellipse";break;case"stadium":o="stadium";break;case"subroutine":o="subroutine";break;case"cylinder":o="cylinder";break;case"group":o="rect";break;case"doublecircle":o="doublecircle";break;default:o="rect"}const U=yield(0,l.r)(e,(0,l.c)());_.setNode(r.id,{labelStyle:E.labelStyle,shape:o,labelText:U,labelType:r.labelType,rx:u,ry:u,class:S,style:E.style,id:r.id,link:r.link,linkTarget:r.linkTarget,tooltip:w.db.getTooltip(r.id)||"",domId:w.db.lookUpDomId(r.id),haveCallback:r.haveCallback,width:r.type==="group"?500:void 0,dir:r.dir,type:r.type,props:r.props,padding:(0,l.c)().flowchart.padding}),l.l.info("setNode",{labelStyle:E.labelStyle,labelType:r.labelType,shape:o,labelText:U,rx:u,ry:u,class:S,style:E.style,id:r.id,domId:w.db.lookUpDomId(r.id),width:r.type==="group"?500:void 0,type:r.type,dir:r.dir,props:r.props,padding:(0,l.c)().flowchart.padding})}})},g=function(t,_,A){return W(this,null,function*(){l.l.info("abc78 edges = ",t);let b=0,f={},w,x;if(t.defaultStyle!==void 0){const n=(0,l.k)(t.defaultStyle);w=n.style,x=n.labelStyle}for(const n of t){b++;const p="L-"+n.start+"-"+n.end;f[p]===void 0?(f[p]=0,l.l.info("abc78 new entry",p,f[p])):(f[p]++,l.l.info("abc78 new entry",p,f[p]));let r=p+"-"+f[p];l.l.info("abc78 new link id to be used is",p,r,f[p]);const S="LS-"+n.start,E="LE-"+n.end,e={style:"",labelStyle:""};switch(e.minlen=n.length||1,n.type==="arrow_open"?e.arrowhead="none":e.arrowhead="normal",e.arrowTypeStart="arrow_open",e.arrowTypeEnd="arrow_open",n.type){case"double_arrow_cross":e.arrowTypeStart="arrow_cross";case"arrow_cross":e.arrowTypeEnd="arrow_cross";break;case"double_arrow_point":e.arrowTypeStart="arrow_point";case"arrow_point":e.arrowTypeEnd="arrow_point";break;case"double_arrow_circle":e.arrowTypeStart="arrow_circle";case"arrow_circle":e.arrowTypeEnd="arrow_circle";break}let i="",u="";switch(n.stroke){case"normal":i="fill:none;",w!==void 0&&(i=w),x!==void 0&&(u=x),e.thickness="normal",e.pattern="solid";break;case"dotted":e.thickness="normal",e.pattern="dotted",e.style="fill:none;stroke-width:2px;stroke-dasharray:3;";break;case"thick":e.thickness="thick",e.pattern="solid",e.style="stroke-width: 3.5px;fill:none;";break;case"invisible":e.thickness="invisible",e.pattern="solid",e.style="stroke-width: 0;fill:none;";break}if(n.style!==void 0){const o=(0,l.k)(n.style);i=o.style,u=o.labelStyle}e.style=e.style+=i,e.labelStyle=e.labelStyle+=u,n.interpolate!==void 0?e.curve=(0,l.n)(n.interpolate,c.lUB):t.defaultInterpolate!==void 0?e.curve=(0,l.n)(t.defaultInterpolate,c.lUB):e.curve=(0,l.n)(L.curve,c.lUB),n.text===void 0?n.style!==void 0&&(e.arrowheadStyle="fill: #333"):(e.arrowheadStyle="fill: #333",e.labelpos="c"),e.labelType=n.labelType,e.label=yield(0,l.r)(n.text.replace(l.e.lineBreakRegex,`
`),(0,l.c)()),n.style===void 0&&(e.style=e.style||"stroke: #333; stroke-width: 1.5px;fill:none;"),e.labelStyle=e.labelStyle.replace("color:","fill:"),e.id=r,e.classes="flowchart-link "+S+" "+E,_.setEdge(n.start,n.end,e,b)}})},V={setConf:$,addVertices:d,addEdges:g,getClasses:function(t,_){return _.db.getClasses()},draw:function(t,_,A,b){return W(this,null,function*(){l.l.info("Drawing flowchart");let f=b.db.getDirection();f===void 0&&(f="TD");const{securityLevel:w,flowchart:x}=(0,l.c)(),n=x.nodeSpacing||50,p=x.rankSpacing||50;let r;w==="sandbox"&&(r=(0,c.Ltv)("#i"+_));const S=w==="sandbox"?(0,c.Ltv)(r.nodes()[0].contentDocument.body):(0,c.Ltv)("body"),E=w==="sandbox"?r.nodes()[0].contentDocument:document,e=new D.T({multigraph:!0,compound:!0}).setGraph({rankdir:f,nodesep:n,ranksep:p,marginx:0,marginy:0}).setDefaultEdgeLabel(function(){return{}});let i;const u=b.db.getSubGraphs();l.l.info("Subgraphs - ",u);for(let k=u.length-1;k>=0;k--)i=u[k],l.l.info("Subgraph - ",i),b.db.addVertex(i.id,{text:i.title,type:i.labelType},"group",void 0,i.classes,i.dir);const o=b.db.getVertices(),U=b.db.getEdges();l.l.info("Edges",U);let M=0;for(M=u.length-1;M>=0;M--){i=u[M],(0,c.Ubm)("cluster").append("text");for(let k=0;k<i.nodes.length;k++)l.l.info("Setting up subgraphs",i.nodes[k],i.id),e.setParent(i.nodes[k],i.id)}yield d(o,e,_,S,E,b),yield g(U,e);const R=S.select(`[id="${_}"]`),N=S.select("#"+_+" g");if(yield(0,C.r)(N,e,["point","circle","cross"],"flowchart",_),l.u.insertTitle(R,"flowchartTitleText",x.titleTopMargin,b.db.getDiagramTitle()),(0,l.o)(e,R,x.diagramPadding,x.useMaxWidth),b.db.indexNodes("subGraph"+M),!x.htmlLabels){const k=E.querySelectorAll('[id="'+_+'"] .edgeLabel .label');for(const P of k){const B=P.getBBox(),m=E.createElementNS("http://www.w3.org/2000/svg","rect");m.setAttribute("rx",0),m.setAttribute("ry",0),m.setAttribute("width",B.width),m.setAttribute("height",B.height),P.insertBefore(m,P.firstChild)}}Object.keys(o).forEach(function(k){const P=o[k];if(P.link){const B=(0,c.Ltv)("#"+_+' [id="'+k+'"]');if(B){const m=E.createElementNS("http://www.w3.org/2000/svg","a");m.setAttributeNS("http://www.w3.org/2000/svg","class",P.classes.join(" ")),m.setAttributeNS("http://www.w3.org/2000/svg","href",P.link),m.setAttributeNS("http://www.w3.org/2000/svg","rel","noopener"),w==="sandbox"?m.setAttributeNS("http://www.w3.org/2000/svg","target","_top"):P.linkTarget&&m.setAttributeNS("http://www.w3.org/2000/svg","target",P.linkTarget);const j=B.insert(function(){return m},":first-child"),z=B.select(".label-container");z&&j.append(function(){return z.node()});const G=B.select(".label");G&&j.append(function(){return G.node()})}}})})}},H=(t,_)=>{const A=y.A,b=A(t,"r"),f=A(t,"g"),w=A(t,"b");return s.A(b,f,w,_)},Q=t=>`.label {
    font-family: ${t.fontFamily};
    color: ${t.nodeTextColor||t.textColor};
  }
  .cluster-label text {
    fill: ${t.titleColor};
  }
  .cluster-label span,p {
    color: ${t.titleColor};
  }

  .label text,span,p {
    fill: ${t.nodeTextColor||t.textColor};
    color: ${t.nodeTextColor||t.textColor};
  }

  .node rect,
  .node circle,
  .node ellipse,
  .node polygon,
  .node path {
    fill: ${t.mainBkg};
    stroke: ${t.nodeBorder};
    stroke-width: 1px;
  }
  .flowchart-label text {
    text-anchor: middle;
  }
  // .flowchart-label .text-outer-tspan {
  //   text-anchor: middle;
  // }
  // .flowchart-label .text-inner-tspan {
  //   text-anchor: start;
  // }

  .node .katex path {
    fill: #000;
    stroke: #000;
    stroke-width: 1px;
  }

  .node .label {
    text-align: center;
  }
  .node.clickable {
    cursor: pointer;
  }

  .arrowheadPath {
    fill: ${t.arrowheadColor};
  }

  .edgePath .path {
    stroke: ${t.lineColor};
    stroke-width: 2.0px;
  }

  .flowchart-link {
    stroke: ${t.lineColor};
    fill: none;
  }

  .edgeLabel {
    background-color: ${t.edgeLabelBackground};
    rect {
      opacity: 0.5;
      background-color: ${t.edgeLabelBackground};
      fill: ${t.edgeLabelBackground};
    }
    text-align: center;
  }

  /* For html labels only */
  .labelBkg {
    background-color: ${H(t.edgeLabelBackground,.5)};
    // background-color: 
  }

  .cluster rect {
    fill: ${t.clusterBkg};
    stroke: ${t.clusterBorder};
    stroke-width: 1px;
  }

  .cluster text {
    fill: ${t.titleColor};
  }

  .cluster span,p {
    color: ${t.titleColor};
  }
  /* .cluster div {
    color: ${t.titleColor};
  } */

  div.mermaidTooltip {
    position: absolute;
    text-align: center;
    max-width: 200px;
    padding: 2px;
    font-family: ${t.fontFamily};
    font-size: 12px;
    background: ${t.tertiaryColor};
    border: 1px solid ${t.border2};
    border-radius: 2px;
    pointer-events: none;
    z-index: 100;
  }

  .flowchartTitleText {
    text-anchor: middle;
    font-size: 18px;
    fill: ${t.textColor};
  }
`;a.d(T,["a",0,Q,"f",0,V])})}]);
}());