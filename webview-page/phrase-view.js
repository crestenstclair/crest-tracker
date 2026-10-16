/* Read-only view calculations over validated native events. Timing membership
 * stays rational; page sizes bound presentation only, never musical content. */
const CrestTrackerPhraseView = (() => {
  "use strict";
  const EVENT_PAGE_SIZE = 64;
  const PIANO_NOTE_GROUP_SIZE = 64;
  const PIANO_NOTE_MIN_WIDTH = 4;
  const PIANO_CONTROL_WIDTH = 24;
  const PIANO_LABEL_WIDTH_PX = 64;
  const { compareBeats, scaleBeat } = CrestTrackerTimeValues;
  const PIANO_SECTION_BEATS = 64n;
  const PIANO_SECTION_STRIDE = PIANO_SECTION_BEATS / 2n;
  const zero = { numerator: 0n, denominator: 1n };
  const add = (a, b) => ({ numerator: BigInt(a.numerator)*BigInt(b.denominator)+BigInt(b.numerator)*BigInt(a.denominator), denominator: BigInt(a.denominator)*BigInt(b.denominator) });
  // Signed quarter-note offset: subtract exactly, approximate only for drawing.
  const beatOffsetAsNumber = (at, origin) => Number(BigInt(at.numerator)*BigInt(origin.denominator)-BigInt(origin.numerator)*BigInt(at.denominator))/Number(BigInt(at.denominator)*BigInt(origin.denominator));
  const minimum = (a, b) => compareBeats(a,b)<0?a:b;
  const maximum = (a, b) => compareBeats(a,b)>0?a:b;

  function drumWindow(events, stepLength, bounds) {
    const steps = new Map(), offgrid = [];
    for (const event of events) {
      const numerator = BigInt(event.at.numerator) * BigInt(stepLength.denominator);
      const denominator = BigInt(event.at.denominator) * BigInt(stepLength.numerator);
      if (numerator < BigInt(bounds.start) * denominator || numerator >= BigInt(bounds.end) * denominator) continue;
      if (numerator % denominator !== 0n) {
        offgrid.push(event);
        continue;
      }
      const step = Number(numerator / denominator);
      if (!steps.has(step)) steps.set(step, []);
      steps.get(step).push(event);
    }
    return { ...bounds, steps, offgrid };
  }

  function pianoSummary(events, cursorPitch) {
    const notes = [], controls = [];
    let low = Math.min(60, cursorPitch), high = Math.max(72, cursorPitch);
    for (const event of events) {
      if (event.note) {
        notes.push(event);
        low = Math.min(low, event.note.pitch);
        high = Math.max(high, event.note.pitch);
      }
      if (event.controls.length) controls.push(event);
    }
    return { notes, controls, low: Math.max(0, low), high: Math.min(127, high) };
  }

  // A section bounds browser geometry, not the phrase. Exact absolute beats are
  // subtracted before conversion to pixels, including beyond Number's range.
  // Sections overlap by half: leaving an edge restores context on both sides,
  // so reversing the cursor does not immediately rebuild the previous section.
  class PianoTimeWindow {
    constructor({ total, anchor = zero, viewportWidth, zoom = 1 }) {
      this.total = total;
      const lastSection = (BigInt(total.numerator)-1n)/(BigInt(total.denominator)*PIANO_SECTION_STRIDE);
      const centered = BigInt(anchor.numerator)/(BigInt(anchor.denominator)*PIANO_SECTION_STRIDE)-1n;
      const requested = centered < 0n ? 0n : centered;
      this.section = requested < lastSection ? requested : lastSection;
      this.start = { numerator: this.section*PIANO_SECTION_STRIDE, denominator: 1n };
      this.end = minimum(total, add(this.start, { numerator: PIANO_SECTION_BEATS, denominator: 1n }));
      this.px = Math.max(320,viewportWidth-PIANO_LABEL_WIDTH_PX)/Math.min(16,beatOffsetAsNumber(total,zero))*zoom;
      this.width = beatOffsetAsNumber(this.end,this.start)*this.px;
    }

    contains(at) { return compareBeats(at,this.start)>=0 && compareBeats(at,this.end)<0; }
    x(at) { return PIANO_LABEL_WIDTH_PX+beatOffsetAsNumber(at,this.start)*this.px; }
    offsetX(at, offset) { return this.x(add(at,offset)); }
    // Constructor anchor for the adjacent section, not that section's start.
    // The constructor owns final phrase-end clamping.
    adjacentSectionAnchor(direction) {
      const targetSection=this.section+BigInt(direction);
      const constructorAnchor=(targetSection+1n)*PIANO_SECTION_STRIDE;
      return { numerator: constructorAnchor<0n?0n:constructorAnchor, denominator: 1n };
    }
    noteGeometry(event) {
      const end=minimum(this.end,add(event.at,event.note.duration));
      const left=this.x(maximum(this.start,event.at));
      const width=Math.max(PIANO_NOTE_MIN_WIDTH,this.x(end)-left);
      return { left, width, right:left+width };
    }
    // Unsnapped presentation time from a millionth-pixel ratio plus the exact origin.
    presentationBeatAtPixel(pixel) {
      const offset={numerator:BigInt(Math.round(Math.max(0,pixel-PIANO_LABEL_WIDTH_PX)*1000000)),denominator:BigInt(Math.round(this.px*1000000))};
      return add(this.start,offset);
    }
    visibleTime(bounds,minimumPixels) {
      return {start:this.presentationBeatAtPixel(bounds.left-minimumPixels),end:Number.isFinite(bounds.right)?minimum(this.end,this.presentationBeatAtPixel(bounds.right)):this.end};
    }
    // Floor to the edit grid and clamp before phrase end. Unrepresentable u64
    // results reject through scaleBeat; this never rewrites source timing.
    editGridBeatAtPixel(pixel, grid) {
      const at=this.presentationBeatAtPixel(pixel);
      const step=BigInt(at.numerator)*BigInt(grid.denominator)/(BigInt(at.denominator)*BigInt(grid.numerator));
      const last=(BigInt(this.total.numerator)*BigInt(grid.denominator)-1n)/(BigInt(this.total.denominator)*BigInt(grid.numerator));
      return scaleBeat(grid,step<last?step:last);
    }
    ticks(grid) {
      const denominator=BigInt(this.start.denominator)*BigInt(grid.numerator);
      const numerator=BigInt(this.start.numerator)*BigInt(grid.denominator);
      let step=(numerator+denominator-1n)/denominator;
      const count=Math.ceil(beatOffsetAsNumber(this.end,this.start)/beatOffsetAsNumber(grid,zero));
      const stride=BigInt(Math.max(1,Math.ceil(count/256)));
      const ticks=[];
      for(let index=0;index<=256;index++,step+=stride){
        // Presentation arithmetic may exceed the wire's u64 limit. Do not
        // validate or serialize decorative ticks as authored time values.
        const at={numerator:BigInt(grid.numerator)*step,denominator:BigInt(grid.denominator)};
        if(compareBeats(at,this.end)>0)break;
        ticks.push({at,left:this.x(at),label:index%4===0});
      }
      return ticks;
    }
  }

  // Retain an exact interval index for the immutable source, independent of
  // section, scroll and zoom. Long notes remain discoverable after their attack.
  class PianoEventIndex {
    #rows = new Map();
    #controls;
    constructor(events) {
      const controls = [];
      for (const [order, event] of events.entries()) {
        if (event.note) {
          const pitch = event.note.pitch;
          if (!this.#rows.has(pitch)) this.#rows.set(pitch, []);
          const row = this.#rows.get(pitch);
          row.push({event,order,group:Math.floor(row.length/PIANO_NOTE_GROUP_SIZE),start:event.at,end:add(event.at,event.note.duration)});
        }
        if(event.controls.length)controls.push({event,order,start:event.at,end:event.at});
      }
      for(const [pitch,entries] of this.#rows)this.#rows.set(pitch,sortAndIndexIntervals(entries));
      this.#controls=sortAndIndexIntervals(controls);
    }

    notesIn(bounds, window) {
      const visible=[],time=window.visibleTime(bounds,PIANO_NOTE_MIN_WIDTH);
      for(let pitch=Math.max(0,Math.ceil(bounds.low));pitch<=Math.min(127,Math.floor(bounds.high));pitch++){
        appendIntersections(visible,this.#rows.get(pitch),time);
      }
      return visible.map(({event,order,group})=>({event,order,group,...window.noteGeometry(event)}))
        .filter(entry=>entry.right>bounds.left&&entry.left<bounds.right).sort((a,b)=>a.order-b.order);
    }

    controlsIn(bounds, window) {
      const visible=[],time=window.visibleTime(bounds,PIANO_CONTROL_WIDTH);
      appendIntersections(visible,this.#controls,time,true);
      return visible.map(({event,order})=>({event,order,left:window.x(event.at),right:window.x(event.at)+PIANO_CONTROL_WIDTH,width:PIANO_CONTROL_WIDTH}))
        .filter(entry=>entry.right>bounds.left&&entry.left<bounds.right).sort((a,b)=>a.order-b.order);
    }
  }

  // Sort the supplied private temporary array in place; return enriched entries.
  function sortAndIndexIntervals(entries) {
    entries.sort((a,b)=>compareBeats(a.start,b.start)||a.order-b.order);
    let end=zero;
    return entries.map(entry=>({...entry,furthestEnd:end=maximum(end,entry.end)}));
  }

  // Append overlaps with [start, end). Point controls include equality at start;
  // note intervals ending exactly at start do not overlap. Both searches agree.
  function appendIntersections(visible, entries, time, includeBoundaryPoints=false) {
    const {start:startBeat,end:endBeat}=time;
    if(!entries?.length||compareBeats(startBeat,endBeat)>=0)return;
    let start=0,end=entries.length;
    while(start<end){
      const middle=Math.floor((start+end)/2),order=compareBeats(entries[middle].furthestEnd,startBeat);
      if(order<0||(!includeBoundaryPoints&&order===0))start=middle+1;else end=middle;
    }
    for(let index=start;index<entries.length&&compareBeats(entries[index].start,endBeat)<0;index++){
      const order=compareBeats(entries[index].end,startBeat);
      if(order>0||(includeBoundaryPoints&&order===0))visible.push(entries[index]);
    }
  }

  // MIDI controls that contribute to one owned-expression lane. Null means
  // no sample for that lane; zero remains an actual controller value.
  function expressionLaneValue(control, kind) {
    const TIMBRE_CC = 74, SUSTAIN_CC = 64;
    switch (kind) {
      case "bend": return control.kind === "bend" ? control.value : null;
      case "pressure": return ["pressure", "poly_pressure"].includes(control.kind) ? control.value : null;
      case "timbre": return control.kind === "cc" && control.number === TIMBRE_CC ? control.value : null;
      case "sustain": return control.kind === "cc" && control.number === SUSTAIN_CC ? control.value : null;
      default: return null;
    }
  }

  // Clip the drawing while retaining the value held at the left boundary.
  // Raw editor points and their order remain in the original event unchanged.
  function pianoExpressionPoints(event, curve, window, amount) {
    const points=[];
    for(const point of curve){
      const value=amount(point.control);if(value===null)continue;
      const x=window.offsetX(event.at,point.offset);
      if(x<PIANO_LABEL_WIDTH_PX){points.length=0;points.push({x:PIANO_LABEL_WIDTH_PX,value});}
      else if(x<=window.width+PIANO_LABEL_WIDTH_PX)points.push({x,value});
      else break;
    }
    return points;
  }

  function pageItems(items, requestedStart) {
    const lastStart = Math.floor(Math.max(0, items.length - 1) / EVENT_PAGE_SIZE) * EVENT_PAGE_SIZE;
    const start = Math.max(0, Math.min(lastStart, Math.floor(requestedStart / EVENT_PAGE_SIZE) * EVENT_PAGE_SIZE));
    const end = Math.min(items.length, start + EVENT_PAGE_SIZE);
    return { start, end, total: items.length, values: items.slice(start, end) };
  }

  return Object.freeze({ drumWindow, pianoSummary, PianoTimeWindow, PianoEventIndex, pianoExpressionPoints, expressionLaneValue, pageItems, EVENT_PAGE_SIZE, PIANO_NOTE_GROUP_SIZE, PIANO_LABEL_WIDTH_PX });
})();
