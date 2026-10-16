/* Immutable native projections; forms contain only uncommitted user input. */
(function(){
  "use strict";
  const {parseBeat,beatLabel,parseTempoMilliBpm,compareBeats,scaleBeat,parseFx,formatFx,nextExpressionSequence}=CrestTrackerEditValues;
  const {unsignedInteger,stringify,encode,decode}=CrestTrackerWireValues;
  const {drumWindow,pianoSummary,PianoTimeWindow,PianoEventIndex,pianoExpressionPoints,expressionLaneValue,pageItems,EVENT_PAGE_SIZE,PIANO_LABEL_WIDTH_PX}=CrestTrackerPhraseView;
  let model=null,formRevision=null,selectingChains=false,timelineRenderKey=null,actionsRenderKey=null,pianoRenderKey=null,gridRenderKey=null,gridFocusKey=null,sourceContextRenderKey=null;
  let lastUiSequence=0,handlingUiInputs=false,controllerFocus=null,controllerText=null;
  let controllerSurface=null;
  const pendingUiInputs=[];
  const expandedPlaybackRows=new Map();
  let playingExpandedRows=new Set();
  const pianoZoomLevels=[0.25,0.5,1,2,4,8];
  let pianoZoom=1,pianoFollow=true,pianoFollowPosition=null,lastPianoCursor=null;
  let pianoViewport=null,pianoViewportFrame=null,pianoAnchor={numerator:0,denominator:1},pianoSectionPhrase=null,pianoSource=null;
  let minimapFollow=false,minimapPendingStart=null;
  let gridWindowKey=null,drumRenderKey=null,drumCursorKey=null;
  let chordPreviewToken=0,chordDraftPreview=null;
  let editPreviewToken=0,pendingEditPreview=null;
  let activeChordLibrary=false,chordLibraryKey=null;
  let activeFileDialog=null;
  let activeSharedToken=null,lastSharedToken=0;
  const retiredFileDialogs=new Set();
  const $=id=>document.getElementById(id);
  const emit=value=>window.__TAURI__.event.emit("crest://host-input",encode(value));
  const button=(text,action,className)=>{const node=document.createElement("button");node.type="button";node.textContent=text;if(className)node.className=className;node.onclick=action;return node;};
  const request=kind=>emit({kind});
  const edit=command=>emit({kind:"edit",revision:formRevision??model.revision,command});
  const beat=(numerator,denominator=1)=>({numerator,denominator});
  const value=b=>Number(b.numerator)/Number(b.denominator);
  const divisionOf=p=>Array.isArray(p.division)?{denominator:p.division[0],notes:p.division[1],in_time_of:p.division[2]}:p.division;
  const grid=p=>{const division=divisionOf(p);return 4/division.denominator*division.in_time_of/division.notes;};
  const gridBeat=(p,steps=1)=>{const division=divisionOf(p);return beat(4*division.in_time_of*steps,division.denominator*division.notes);};
  const phraseEditBeat=(phrase=model.phrase?.value,row=model.row)=>phrase?.form.kind==="piano_roll"?model.pianoControls.at:phrase?gridBeat(phrase,row):beat(0);
  const file=action=>emit({kind:"file",action});
  let feedbackScheduled=0;
  function acknowledgeFeedback(next){
    const capture=next.feedback,status=$("tracker-feedback-status");
    status.hidden=!capture?.enabled;
    if(capture?.enabled)status.textContent=`UI FEEDBACK CAPTURE · ${capture.nativeKeyboardPlaybackSamples} / 100 keyboard playback samples · ${capture.samples} total samples · ${capture.failures} failures · ${next.playing?"PLAYING":"STOPPED"} · Close window to write report`;
    const pending=capture?.pending;
    if(!pending||pending.sequence===feedbackScheduled)return;
    feedbackScheduled=pending.sequence;
    // Two frame callbacks permit a render opportunity between DOM mutation and
    // acknowledgement. This is not GPU presentation or physical display timing.
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
      if(model?.feedback?.pending?.sequence!==pending.sequence||document.visibilityState!=="visible"||$("tracker-dialog").open)return;
      const expected=pending.state;
      if(model.view!==expected.view||model.revision!==expected.revision||model.playing!==expected.playing)return;
      const cell=feedbackTarget(expected.focus);if(!cell)return;
      const bounds=cell.getBoundingClientRect(),clip=$("tracker-grid").getBoundingClientRect();
      if(bounds.width<=0||bounds.height<=0||bounds.left<Math.max(0,clip.left)||bounds.top<Math.max(0,clip.top)||bounds.right>Math.min(innerWidth,clip.right)+1||bounds.bottom>Math.min(innerHeight,clip.bottom)+1)return;
      if(["piano","piano_selection","piano_edit"].includes(expected.focus.kind)&&bounds.left<pianoLabelsRight(cell.closest(".piano-roll")))return;
      if(["drum","drum_edit"].includes(expected.focus.kind)&&bounds.left<$("tracker-grid").querySelector(".drum-grid th:first-child").getBoundingClientRect().right)return;
      // Verify the focus target is actually exposed, not covered by another UI.
      const hit=document.elementFromPoint(bounds.left+bounds.width/2,bounds.top+bounds.height/2);
      const cursorFeedback=expected.focus.kind==="piano"||(expected.focus.kind==="piano_selection"&&expected.focus.selected===null);
      const paintedRegion=cursorFeedback?cell.closest(".piano-roll"):cell;
      if(hit!==paintedRegion&&!paintedRegion.contains(hit))return;
      emit({kind:"feedback_painted",sequence:pending.sequence,state:expected,bounds:[bounds.x,bounds.y,bounds.width,bounds.height],viewport:[innerWidth,innerHeight]});
    }));
  }
  function feedbackTarget(focus){
    if(focus.kind==="piano_edit")return pianoEditFeedbackTarget(focus);
    if(focus.kind==="piano_selection"){
      const selected=focus.selected===null?[]:[focus.selected];
      if(model.phrase?.id!==focus.phrase||model.phrase.value.form.kind!=="piano_roll"||stringify(model.selectedEvents)!==stringify(selected)||model.pianoControls.pitch!==focus.pitch||compareBeats(model.pianoControls.at,focus.at)!==0)return null;
      if(focus.selected===null)return $("tracker-grid").querySelector(".piano-cursor-point");
      return Array.from($("tracker-grid").querySelectorAll(".piano-note.selected[aria-pressed=true]"))
        .find(note=>note.dataset.eventId===String(focus.selected))??null;
    }
    if(["drum","drum_edit"].includes(focus.kind)){
      const lane=model.drumLanes?.[focus.lane];
      if(model.phrase?.id!==focus.phrase||model.phrase.value.form.kind!=="drum"||model.row!==focus.step||model.column!==focus.lane||lane?.pitch!==focus.pitch||stringify(lane.target)!==stringify(focus.target))return null;
      const cell=$("tracker-grid").querySelector(".drum-hit.focused");
      if(focus.kind==="drum_edit"){
        const count=focus.hits.length,label=count>1?String(count):count?"●":"·";
        if(cell?.dataset.hits!==stringify(focus.hits)||cell.textContent!==label||cell.classList.contains("active")!==(count>0)||cell.getAttribute("aria-pressed")!==String(count>0))return null;
      }
      return Number(cell?.dataset.step)===focus.step&&Number(cell?.dataset.lane)===focus.lane&&cell.getAttribute("aria-current")==="true"?cell:null;
    }
    if(focus.kind==="piano"){
      if(model.phrase?.id!==focus.phrase||model.phrase.value.form.kind!=="piano_roll"||model.pianoControls.pitch!==focus.pitch||compareBeats(model.pianoControls.at,focus.at)!==0)return null;
      const cursor=$("tracker-grid").querySelector(".piano-cursor-point");
      return cursor?.dataset.at===beatKey(focus.at)&&Number(cursor.dataset.pitch)===focus.pitch?cursor:null;
    }
    if(focus.kind==="timeline")return timelineFeedbackTarget(focus);
    if(focus.kind!=="grid"||model.row!==focus.row||model.column!==focus.column)return null;
    const cells=$("tracker-grid").querySelectorAll("button.focused[data-grid-row]");
    if(cells.length!==1)return null;
    const cell=cells[0];
    return Number(cell.dataset.gridRow)===focus.row&&Number(cell.dataset.gridColumn)===focus.column&&cell.dataset.label===focus.label&&cell.getAttribute("aria-selected")==="true"?cell:null;
  }
  function timelineFeedbackTarget(focus){
    if(!["expanded","minimap"].includes(model.view))return null;
    const span=selectedTimelineSpan();
    if(!span||span.phrase!==focus.phrase||!sameOccurrence(span.occurrence,focus.occurrence)||compareBeats(span.start,focus.start)!==0)return null;
    const controls=Array.from($("tracker-grid").querySelectorAll("button.focused[aria-selected=true]"));
    if(model.view==="minimap")return focus.row===null&&controls.length===1&&controls[0].dataset.spanKey===spanKey(span)?controls[0]:null;
    if(model.timelineRow!==focus.row||controls.length!==6||!focus.at||focus.cells.length!==6)return null;
    for(const control of controls){
      if(control.dataset.spanKey!==spanKey(span)||Number(control.dataset.sourceRow)!==focus.row||control.dataset.rowAt!==beatKey(focus.at)||control.dataset.value!==focus.cells[Number(control.dataset.column)])return null;
    }
    return controls.find(control=>control.dataset.column==="0")??null;
  }
  function pianoEditFeedbackTarget(focus){
    if(model.phrase?.id!==focus.phrase||model.phrase.value.form.kind!=="piano_roll"||!focus.notes.length)return null;
    const selected=new Set(model.selectedEvents),view=pianoViewport,metrics=pianoMetrics(model.phrase.value);
    if(!view||selected.size!==focus.notes.length)return null;
    const events=new Map(model.phrase.value.events.map(event=>[event.id,event]));
    // Equivalent native projections have new JS objects. Retained DOM nodes
    // belong to the content revision, so correlate them by stable source ID.
    const nodes=new Map(Array.from(view.noteNodes).filter(([event])=>view.storedEvents.has(event)).map(([event,node])=>[event.id,node]));
    const clip=$("tracker-grid").getBoundingClientRect();let first=null;
    for(const expected of focus.notes){
      const event=events.get(expected.id),note=event?.note,node=nodes.get(expected.id);
      if(!selected.has(expected.id)||!note||!node||compareBeats(event.at,expected.at)!==0||compareBeats(note.duration,expected.duration)!==0||note.pitch!==expected.pitch||note.velocity!==expected.velocity||note.release_velocity!==expected.release_velocity)return null;
      const {left,width}=metrics.noteGeometry(event);
      if(Math.abs(parseFloat(node.style.left)-left)>1/64||Math.abs(parseFloat(node.style.top)-(36+(view.high-expected.pitch)*22))>1/64||Math.abs(parseFloat(node.style.width)-width)>1/64||node.getAttribute("aria-label")!==pianoNoteLabel(event,true)||node.getAttribute("aria-pressed")!=="true")return null;
      // Every selected note must be painted and exposed. Offscreen/covered
      // members cannot be credited by acknowledging only the first note.
      const bounds=node.getBoundingClientRect();
      if(bounds.width<=0||bounds.height<=0||bounds.left<Math.max(0,clip.left,pianoLabelsRight(view.roll))||bounds.top<Math.max(0,clip.top)||bounds.right>Math.min(innerWidth,clip.right)+1||bounds.bottom>Math.min(innerHeight,clip.bottom)+1)return null;
      const hit=document.elementFromPoint(bounds.left+bounds.width/2,bounds.top+bounds.height/2);
      if(hit!==node&&!node.contains(hit))return null;
      first??=node;
    }
    return first;
  }
  function render(next){
    // Explicit cursor movement owns the viewport until Follow is resumed.
    if(model?.view==="phrase"&&next.view==="phrase"&&model.phrase?.id===next.phrase?.id&&next.phrase?.value.form.kind==="piano_roll"
      &&(model.pianoControls.pitch!==next.pianoControls.pitch||compareBeats(model.pianoControls.at,next.pianoControls.at)!==0))pianoFollow=false;
    if(model?.view==="expanded"&&next.view!=="expanded"){expandedPlaybackRows.clear();playingExpandedRows.clear();}
    if(model?.view!==next.view||model?.phrase?.value.form.kind!==next.phrase?.value.form.kind){pianoRenderKey=null;gridRenderKey=null;drumRenderKey=null;drumCursorKey=null;}
    model=next;const synth=next.view==="instruments";
    updateChordPreview(next.chordPreview);
    updateEditPreview(next.editPreview);
    updateChordLibrary();
    if(next.view!=="phrase"||next.phrase?.value.form.kind!=="piano_roll"){pianoFollowPosition=null;lastPianoCursor=null;pianoViewport=null;pianoSectionPhrase=null;pianoSource=null;}
    if(!["expanded","minimap"].includes(next.view))timelineRenderKey=null;
    $("synth-root").hidden=!synth;$("tracker-root").hidden=synth;$("tracker-return").hidden=!synth;
    renderFileDialog(next.fileDialog);
    renderSharedEdit(next.sharedEdit);
    if(synth){consumeUiInputs(next.uiInputs||[]);return;}
    renderIdentity();
    updateRecordSetupInput();
    const captureState=next.capture?.state?.state;
    $("tracker-transport").textContent=captureState==="recording"?"RECORDING":captureState==="armed"?"ARMED":captureState==="waiting"?"WAITING":next.playing?"▶ PLAYING":"STOPPED";
    $("tracker-status").textContent=(next.status===next.playbackFault?"":next.status)+(next.playbackPending?" · EDIT PENDING · Waiting for musical boundary":"");
    const playbackFault=$("tracker-playback-fault");playbackFault.hidden=!next.playbackFault;
    const faultText=next.playbackFault?"PLAYBACK STOPPED · "+next.playbackFault:"";
    if(playbackFault.textContent!==faultText)playbackFault.textContent=faultText;
    const settings=next.view==="settings";
    $("tracker-grid").hidden=settings;$("tracker-settings").hidden=!settings;
    $("tracker-timeline-context").hidden=!["expanded","minimap"].includes(next.view);
    renderActions();
    renderGridWindow();
    renderSourceContext();
    if(settings)renderSettings();else if(["expanded","minimap"].includes(next.view))renderTimeline();else if(next.view==="phrase"&&next.phrase?.value.form.kind==="piano_roll")renderPiano();else if(next.view==="phrase"&&next.phrase?.value.form.kind==="drum")renderDrum();else renderGrid();
    if(next.view==="song")updateSongPlayback();
    if(["chain","phrase"].includes(next.view))updateGridPlayback();
    restoreSurfaceFocus();
    consumeUiInputs(next.uiInputs||[]);
  }
  function contextName(){
    if(model.view==="minimap")return "SONG / MINIMAP";
    if(model.view==="expanded"&&currentViewGroup())return "GROUP / "+currentViewGroup();
    const id=model.view==="chain"?model.selectedChain?.id:model.view==="phrase"?model.phrase?.id:null;
    return model.view.toUpperCase()+(id===null||id===undefined?"":" "+String(id).padStart(2,"0"));
  }
  function renderIdentity(){
    if(!model)return;
    const dialog=$("tracker-dialog"),sheet=dialog.open&&dialog.classList.contains("piano-sheet");
    $("tracker-context-name").textContent=sheet?"PHRASE "+model.phrase.id:contextName();
    $("tracker-title").textContent="/ "+(sheet?dialog.dataset.title:model.title)+(model.dirty?" *":"");
    $("tracker-subtitle").textContent=sheet?dialog.dataset.subtitle:model.subtitle;
    renderFooter(sheet);
  }
  function renderFooter(sheet){
    $("tracker-breadcrumb").textContent=sheet?"PIANO ROLL":contextName()+" / TRACK "+String(model.track+1).padStart(2,"0");
    const hints={song:"SHIFT ↑ Settings · SHIFT ↓ Mixer · SHIFT + BACK Menu · PLAY",
      chain:"SHIFT → Phrase · SHIFT ← Song · EDIT + Direction Edit · PLAY",
      settings:"D-PAD Browse · CONFIRM Open · SHIFT ↓ Quick Jump · BACK Song",
      expanded:"SHIFT ← Song · SHIFT ↓ Quick Jump · CONFIRM Source · PLAY",
      minimap:"SHIFT ← Song · SHIFT ↓ Quick Jump · CONFIRM Source · PLAY"};
    const phrase=model.phrase?.value.form.kind;
    hints.phrase=phrase==="drum"?"↑/↓ Lane · ←/→ Step · CONFIRM Toggle · EDIT ↑ Settings · PLAY":
      phrase==="piano_roll"?"EDIT ↑ Notes · EDIT ←/→ Nudge · SHIFT ↑ Settings · SHIFT ← Chain":
      "EDIT ↑/↓ Pitch · EDIT ←/→ Octave · SHIFT ↑ Settings · SHIFT ← Chain";
    $("tracker-hints").textContent=sheet?"D-PAD Browse · CONFIRM Edit / Apply · BACK Roll":hints[model.view]||"SHIFT ↓ Quick Jump";
  }
  function showPianoSheet(title,subtitle){
    const dialog=$("tracker-dialog");dialog.classList.add("piano-sheet");dialog.dataset.title=title;dialog.dataset.subtitle=subtitle;
    renderIdentity();
  }
  function renderActions(){
    const key=stringify([model.view,model.resourceRepair,model.sharedReferences!==null,model.selectedEvents?.length,model.phrase?.value.form.kind,model.phraseKey,model.pitchIntervals,model.chromaticEntry,model.viewSelection,currentViewGroup(),model.timeline?.bars?.[0]?.meter,model.capture?.state?.state,!!model.capture?.draft,!!model.capture?.held,model.previewBars,selectingChains,pianoFollow,pianoZoom,minimapFollow,model.view==="song"?songWindow():null]);
    if(key===actionsRenderKey)return;actionsRenderKey=key;
    const nav=$("tracker-actions");nav.replaceChildren();nav.classList.remove("piano-actions","song-actions","sequence-actions","timeline-actions");nav.setAttribute("aria-label",model.view+" actions");
    if(model.resourceRepair)nav.append(button("REPAIR ASSETS",resourceRepairDialog),button("CANCEL OPEN",()=>emit({kind:"repair_resource",index:0,action:"cancel"})));
    nav.append(button("SONG",()=>emit({kind:"view",view:"song"}),"muted"),button("BACK",()=>request("back")),button("OPEN / EDIT",()=>model.view==="phrase"?model.phrase?.value.form.kind==="piano_roll"?eventChooser("edit"):contextEdit():request("confirm")));
    nav.append(button("SETTINGS",()=>emit({kind:"view",view:"settings"})),button("UNDO",()=>request("undo")),button("REDO",()=>request("redo")),button("SAVE",()=>file("save")),button("MENU",menu),button("SONG FILE",songFileMenu),button("QUICK JUMP",quickJump));
    nav.append(button("CHORD BANK",chordBankDialog),button("CHORD LIBRARY",chordLibraryDialog),button("PLAY FROM / AUDITION",transportDialog));
    if(model.sharedReferences)nav.append(button("SHARED REFERENCES",sharedReferencesDialog));
    nav.append(button("HELP",helpDialog));
    if(["song","expanded","minimap"].includes(model.view))nav.append(button("CHAINS",()=>emit({kind:"view",view:"song"})),button("EXPANDED"+(model.expandedSelection?.length?" ("+model.expandedSelection.length+")":""),()=>emit({kind:"view",view:"expanded"})),button("MINIMAP",()=>emit({kind:"view",view:"minimap"})));
    if(["song","expanded","minimap"].includes(model.view))nav.append(button("VIEW GROUPS",viewGroupsDialog));
    if(model.view==="song")nav.append(button("ARRANGE",arrangeDialog),button("MOVE SLOT",moveSlotDialog),button("TRACK SETTINGS",trackDialog),button(selectingChains?"MULTISELECT ON":"MULTISELECT",()=>{selectingChains=!selectingChains;renderActions();renderGrid();}),button("SELECT CURRENT CHAIN",toggleCurrentChain),button("CHOOSE CHAINS",chainSelectionDialog));
    if(["expanded","minimap"].includes(model.view))for(const bars of [8,32,64])nav.append(button(bars+" BARS",()=>emit({kind:"preview_bars",bars}),model.previewBars===bars?"selected":"muted"));
    if(["expanded","minimap"].includes(model.view))nav.append(button("EARLIER",()=>shiftTimeline(-1)),button("LATER",()=>shiftTimeline(1)),button("GO TO BEAT",timelinePositionDialog));
    if(model.view==="minimap")nav.append(button("FOLLOW "+(minimapFollow?"ON":"OFF"),()=>{minimapFollow=!minimapFollow;renderActions();followMinimap();}));
    if(model.view==="chain")nav.append(button("EDIT PLACEMENT",()=>chainPlacementDialog()),button("ADD / REUSE PHRASE",chainDialog),button("CHAIN SETTINGS",chainSettingsDialog),button("MAKE CHAIN UNIQUE",makeChainUnique));
    if(model.view==="phrase"){
      nav.append(button(model.selectedEvents?.length?"DELETE SELECTED":"CLEAR",()=>model.selectedEvents?.length?selectionRequest({kind:"delete"}):request("clear")),button("PHRASE SETTINGS",phraseDialog),button("PHRASE KEY",()=>scaleDialog(true)),button("PITCH DISPLAY",pitchDisplayDialog),button("RECORD SETUP",recordSetup));
      const state=model.capture?.state?.state||"idle";
      if(state==="armed")nav.append(button("● RECORD",()=>capture("record")));
      if(state==="recording"||state==="waiting")nav.append(button("■ END TAKE",()=>capture("end")));
      if(state==="interrupted"&&model.capture?.held)nav.append(button("FINALIZE HELD NOTES",()=>capture("end")));
      if(model.capture?.draft&&["idle","interrupted"].includes(state))nav.append(button("COMMIT TAKE",()=>capture("commit")),button("SAVE TAKE AS NEW PHRASE",()=>capture("save_as_new_phrase")));
      if(state!=="idle"||model.capture?.draft)nav.append(button("DISCARD TAKE",()=>capture("discard")));
      if(["euclidean","random"].includes(model.phrase.value.form.kind))nav.append(button("GENERATOR",generatorDialog),button("BAKE TO PIANO ROLL",()=>bakeGenerator("piano_roll")),button("BAKE TO TRACKER",()=>bakeGenerator("tracker")));
      if(model.phrase.value.form.kind==="drum")nav.append(button("ADD DRUM LANE",()=>drumLaneDialog()),button("LANE SETTINGS",()=>drumLaneDialog(drumLanes()[model.column])));
      if(model.phrase.value.form.kind==="piano_roll")nav.append(button("INSERT NOTE",()=>noteDialog(model.row,null,noteName(model.pianoControls.pitch),true,model.pianoControls.at)),button("CHOOSE NOTE / EVENT",()=>eventChooser()),button("PREVIOUS NOTE",()=>cycleNote(-1)),button("NEXT NOTE",()=>cycleNote(1)),button("SELECT ALL",selectAllEvents),button("SELECT RANGE",rangeSelectionDialog),button("DUPLICATE",()=>selectionDialog("duplicate")),button("MOVE",()=>selectionDialog("move")),button("TRANSPOSE",()=>selectionDialog("transpose")),button("QUANTIZE",()=>selectionDialog("quantize")),button("SELECTION ("+(model.selectedEvents?.length||0)+")",selectionMenu),button("RAW MIDI",()=>performanceDialog()));
      if(model.phrase.value.form.kind==="piano_roll"){
        const follow=button("FOLLOW "+(pianoFollow?"ON":"OFF"),togglePianoFollow);follow.setAttribute("aria-pressed",String(pianoFollow));
        nav.append(button("EDIT GRID / STICK VALUE",pianoControlsDialog),button("ZOOM OUT",()=>changePianoZoom(-1)),button("ZOOM IN",()=>changePianoZoom(1)),button("RESET ZOOM ("+Math.round(pianoZoom*100)+"%)",()=>setPianoZoom(1)),follow);
      }
    }
    if(model.view==="song")groupSongActions(nav);
    if(model.view==="phrase"&&model.phrase?.value.form.kind==="piano_roll")groupPianoActions(nav);
    else if(model.view==="chain"||model.view==="phrase")groupSequenceActions(nav);
    else if(["expanded","minimap"].includes(model.view))groupTimelineActions(nav);
  }
  function currentViewGroup(){
    const selection=stringify(model.viewSelection);
    return Object.entries(model.viewGroups||{}).find(([,group])=>stringify(group.selections)===selection)?.[0]||null;
  }
  function songViewDialog(){
    form("Song view",[{name:"view",label:"View",value:model.view,options:[{value:"song",label:"Chains"},{value:"expanded",label:"Expanded selected chains"},{value:"minimap",label:"Song Minimap"}]}],values=>emit({kind:"view",view:values.view}));
  }
  function groupTimelineActions(nav){
    const controls=[...nav.children],primary=[];
    if(model.view==="minimap")primary.push(button("VIEW / MINIMAP ▾",songViewDialog));
    const context=document.createElement("span");context.className="timeline-context";
    context.textContent=model.view==="expanded"?`MULTISELECT / ${model.viewSelection?.length||0} CHAINS${currentViewGroup()?" · VIEW GROUP / "+currentViewGroup():""} · ALL PHRASES`:`${model.previewBars} BARS · ${model.timeline?.bars?.[0]?.meter.join("/")||""} · ALL ${model.tracks.length} TRACKS`;
    primary.push(context,button(`ZOOM ${model.previewBars} BARS`,()=>form("Timeline window",[{name:"bars",label:"Visible bars",value:model.previewBars,type:"number",min:1,max:64}],values=>emit({kind:"preview_bars",bars:Number(values.bars)}))));
    const follow=controls.find(control=>control.textContent.startsWith("FOLLOW "));
    if(follow){follow.setAttribute("aria-pressed",String(minimapFollow));primary.push(follow);}
    nav.replaceChildren(...primary,actionsMenu(controls.filter(control=>control!==follow&&!/^\d+ BARS$/.test(control.textContent))));nav.classList.add("timeline-actions");
  }
  function groupSequenceActions(nav){
    const controls=[...nav.children];
    let context;
    if(model.view==="chain"){
      context=document.createElement("span");context.className="sequence-context";
      context.textContent="ORDERED PHRASE REFERENCES · REPEATS APPLY AT EACH PLACEMENT";
    }else{
      context=button(`${model.phraseKey||model.key} · ${model.pitchIntervals?"INTERVALS":"NOTES"} · ${model.chromaticEntry?"CHROMATIC":"SCALE ONLY"} ›    |    PER-NOTE INSTRUMENT / MIDI CHANNEL`,pitchDisplayDialog,"sequence-context");
    }
    nav.replaceChildren(context,actionsMenu(controls));nav.classList.add("sequence-actions");
  }
  function actionsMenu(controls,label="MORE ACTIONS"){
    const more=document.createElement("details");more.className="tracker-more";
    const summary=document.createElement("summary");summary.textContent=label;
    const secondary=document.createElement("div");secondary.className="tracker-secondary-actions";
    secondary.append(...controls);secondary.addEventListener("click",()=>{more.open=false;});more.append(summary,secondary);
    return more;
  }
  function songWindow(){
    const width=$("tracker-grid").clientWidth;
    const size=width>=1176?8:width>=616?4:2;
    const start=Math.floor(model.column/size)*size;
    return {start,end:Math.min(start+size,model.tracks.length),size};
  }
  function groupSongActions(nav){
    const controls=[...nav.children],find=label=>controls.find(control=>control.textContent===label);
    const viewport=songWindow(),number=index=>String(index).padStart(2,"0");
    const view=button("VIEW / CHAINS ▾",songViewDialog,"muted");
    const tracks=button(`TRACKS ${number(viewport.start+1)}–${number(viewport.end)} OF ${model.tracks.length} ›`,()=>emit({kind:"focus",row:model.row,column:(model.column+viewport.size)%model.tracks.length}),"muted");
    tracks.setAttribute("aria-label",tracks.textContent+" · Next track bank");
    const multiselect=find(selectingChains?"MULTISELECT ON":"MULTISELECT");multiselect.setAttribute("aria-pressed",String(selectingChains));
    const grouped=[find("SELECT CURRENT CHAIN"),find("CHOOSE CHAINS"),find("VIEW GROUPS"),controls.find(control=>control.textContent.startsWith("EXPANDED"))];
    const remaining=controls.filter(control=>control!==multiselect&&!grouped.includes(control)&&!["SONG","CHAINS","MINIMAP"].includes(control.textContent));
    nav.replaceChildren(view,tracks,multiselect,actionsMenu(grouped,"GROUP ACTIONS"),actionsMenu(remaining));
    nav.classList.add("song-actions");
  }
  function renderSourceContext(){
    const node=$("tracker-selection");node.hidden=!["song","chain","phrase"].includes(model.view)||model.phrase?.value.form.kind==="piano_roll"&&model.view==="phrase";
    if(node.hidden)return;
    const key=stringify([model.revision,model.view,model.row,model.column,model.selectedChain,model.phrase?.id,model.rows[model.row-model.rowStart],model.sharedReferences,model.drumLanes]);
    if(key===sourceContextRenderKey)return;sourceContextRenderKey=key;
    node.replaceChildren();
    if(model.view==="chain"){
      node.textContent=model.phrase?`PHRASE ${String(model.phrase.id).padStart(2,"0")} / ${model.phrase.value.name}    |    SHIFT → Open phrase    |    Linked content is reused`:"NEW PLACEMENT · Confirm to create a phrase; More Actions to reuse one";
      return;
    }
    if(model.view==="phrase"){
      const row=model.rows[model.row-model.rowStart],references=model.sharedReferences?.references?.length||0;
      const lane=model.phrase?.value.form.kind==="drum"?model.drumLanes?.[model.column]:null;
      const label=lane?`STEP ${model.row+1} / ${noteName(lane.pitch)}    |    ${targetLabel(lane.target)}`:`ROW ${String(model.row+1).padStart(2,"0")} / ${row?.[0]||"—"}    |    ${row?.[1]||"—"}`;
      node.append(label+"    |    ",button(`${references} REFERENCE${references===1?"":"S"} ›`,sharedReferencesDialog));
      return;
    }
    const selected=model.selectedChain;
    if(!selected){node.textContent="EMPTY SLOT · Confirm to create or choose a chain";return;}
    const references=model.tracks.reduce((count,track)=>count+track.slots.filter(use=>use?.chain===selected.id).length,0);
    const phrases=selected.value.phrases.length;
    node.textContent=`CHAIN ${selected.id} · ${selected.value.name}    |    ${phrases} PHRASE${phrases===1?"":"S"}    |    ${references} REFERENCE${references===1?"":"S"}`;
  }
  function updateGridPlayback(){
    const track=model.tracks[model.track],usage=track?.slots[model.slot],heads=model.playHeads||[];
    for(const row of $("tracker-grid").querySelectorAll(".tracker-table tbody tr[data-row]")){
      const index=Number(row.dataset.row);
      const playing=model.playing&&(model.view==="phrase"?model.playRow===index:heads.some(head=>head.occurrence.track===track?.id&&head.occurrence.chain_use===usage?.id&&head.occurrence.phrase_use===model.selectedChain?.value.phrases[index]?.id));
      row.classList.toggle("playing",playing);row.firstElementChild.textContent=(playing?"▶ ":"")+String(index+1).padStart(2,"0");
    }
  }
  function updateSongPlayback(){
    const active=new Set((model.playHeads||[]).map(head=>`${head.occurrence.track}:${head.occurrence.chain_use}`));
    for(const cell of $("tracker-grid").querySelectorAll("td[data-chain-use]")){
      const playing=model.playing&&active.has(cell.dataset.chainUse),control=cell.firstElementChild;
      cell.classList.toggle("playing",playing);
      control.textContent=(playing?"▶ ":"")+control.dataset.value;
      control.setAttribute("aria-label",control.dataset.label+(playing?" · Playing":""));
    }
  }
  function groupPianoActions(nav){
    const controls=[...nav.children],find=label=>controls.find(control=>control.textContent===label);
    const primary=[find("■ END TAKE")||find("● RECORD")||find("FINALIZE HELD NOTES")||find("COMMIT TAKE"),find("RECORD SETUP")].filter(Boolean);
    const more=actionsMenu(controls.filter(control=>!primary.includes(control)));
    const status=document.createElement("span");status.className="piano-capture-state";
    nav.replaceChildren(...primary,button(`EDIT NOTES · ${model.selectedEvents?.length||0} SELECTED`,pianoEditNotes),more,status);
    nav.classList.add("piano-actions");
  }
  function pianoEditNotes(){
    const source={revision:model.revision,occurrence:model.occurrence};
    const note=model.phrase.value.events.find(event=>model.selectedEvents?.includes(event.id)&&event.note);
    const chooseNote=()=>note?noteDialog(value(note.at)/grid(model.phrase.value),note):eventChooser("edit");
    form("NOTE ACTIONS · ALL AVAILABLE WITH GAMEPAD",[],()=>{},true);
    const count=model.selectedEvents?.length||0;
    showPianoSheet("EDIT NOTES",count===1&&note?"1 NOTE SELECTED · "+noteName(note.note.pitch):`${count} EVENT${count===1?"":"S"} SELECTED`);$("tracker-form").querySelector("footer").hidden=true;
    const rows=[
      ["PITCH / START / LENGTH",note?`${noteName(note.note.pitch)} · ${beatLabel(note.at)} BEATS · ${beatLabel(note.note.duration)} BEATS`:"CHOOSE NOTE / EVENT",chooseNote],
      ["VELOCITY / RELEASE / TARGET",note?`${note.note.velocity} / ${note.note.release_velocity} · ${targetLabel(note.target)}`:"CHOOSE NOTE / EVENT",chooseNote],
      ["QUANTIZE SELECTED",`${beatLabel(model.pianoControls.grid)} BEAT GRID · REVIEW BEFORE APPLY`,()=>selectionDialog("quantize")],
      ["EXPRESSION","BEND · PRESSURE · TIMBRE · SUSTAIN · POINT EDITOR",()=>note?performanceDialog(note):eventChooser("raw")],
      ["SELECTION / CLIPBOARD","RANGE · ALL · COPY · PASTE · DUPLICATE · DELETE",selectionMenu],
      ["GRID / ZOOM / RECORDING","FOCUS PHRASE TOOLBAR · MORE ACTIONS",focusControls],
      ["UNDO / REDO","UNDO LAST EDIT · REDO",()=>form("Undo / Redo",[{name:"action",label:"Action",value:"undo",options:[{value:"undo",label:"Undo last edit"},{value:"redo",label:"Redo"}]}],values=>request(values.action))]
    ];
    const container=$("tracker-fields");container.replaceChildren();
    for(const [label,description,action] of rows){const row=setting(label,description,()=>{if(model.revision!==source.revision||!sameOccurrence(model.occurrence,source.occurrence)){$("tracker-form-error").textContent="Phrase changed; reopen Edit Notes to use the current selection.";return;}hideDialog();queueMicrotask(action);});row.querySelector("button").setAttribute("aria-label",label+" · "+description);container.append(row);}
    dialogNote("Nudge and quantize move expression with notes. Grid changes never retime recorded music.");
    dialogNote("More Actions also exposes Insert, Grid / Triplets, Zoom / Scroll, FX 1–3 and Record setup.");
    focusController(container.querySelector("button"));
  }
  function targetLabel(target){return target.kind==="instrument"?String(target.id).padStart(2,"0")+" / "+(model.instruments[target.id]?.name||"UNRESOLVED")+" · CH "+String(target.channel).padStart(2,"0"):(target.endpoint?"EXTERNAL / "+target.endpoint:"INTERNAL")+" · CH "+String(target.channel).padStart(2,"0");}
  function renderGridWindow(){
    const node=$("tracker-grid-window");
    if(model.view==="phrase"&&model.phrase?.value.form.kind==="piano_roll")return;
    const isGrid=["song","chain","phrase"].includes(model.view)&&!(model.view==="phrase"&&model.phrase?.value.form.kind==="piano_roll");
    node.hidden=!isGrid||model.rowCount<=model.rowPageSize;
    if(node.hidden){gridWindowKey=null;return;}
    const editableRows=model.view==="song"?model.tracks[model.track].slots.length+1:model.rowCount;
    const start=model.rowStart,end=start+model.rows.length;
    const key=stringify([model.view,start,end,model.rowCount,editableRows]);
    if(key===gridWindowKey)return;gridWindowKey=key;
    const label=document.createElement("span");label.textContent=`ROWS ${start+1}–${end} OF ${model.rowCount}`;
    const focus=row=>emit({kind:"focus",row,column:model.column});
    const previous=button("PREVIOUS ROWS",()=>focus(Math.max(0,start-model.rowPageSize)));previous.disabled=start===0;
    const next=button("NEXT ROWS",()=>focus(end));next.disabled=end>=editableRows;
    const jump=button("GO TO ROW",()=>form("Go to row",[{name:"row",label:`Row · 1–${editableRows}${model.view==="song"?" on selected track":""}`,value:model.row+1,type:"number",min:1,max:editableRows}],values=>{
      const row=Number(values.row);if(!Number.isSafeInteger(row)||row<1||row>editableRows)throw Error("Choose an existing row or the append row");focus(row-1);
    }));
    node.replaceChildren(label,previous,next,jump);
  }
  function renderGrid(){
    const viewport=model.view==="song"?songWindow():{start:0,end:model.columns.length};
    const key=stringify([model.revision,model.view,model.rowStart,model.rows,model.columns,model.expandedSelection,selectingChains,viewport]);
    if(key===gridRenderKey){updateGridFocus();return;}gridRenderKey=key;gridFocusKey=null;
    const gridNode=$("tracker-grid"),oldLeft=gridNode.scrollLeft,oldTop=gridNode.scrollTop;
    gridNode.dataset.mode=model.view;timelineRenderKey=null;
    const table=document.createElement("table");table.className="tracker-table "+(model.view==="phrase"?"phrase":model.view==="chain"?"chain":"song");
    table.setAttribute("aria-rowcount",String(Math.max(model.view==="chain"?12:0,model.rowCount)+1));
    const head=document.createElement("thead"),row=document.createElement("tr");
    for(const name of ["ROW",...model.columns.slice(viewport.start,viewport.end)]){const cell=document.createElement("th");cell.scope="col";cell.textContent=name;row.append(cell);}head.append(row);table.append(head);
    const body=document.createElement("tbody");
    model.rows.forEach((values,index)=>{const r=index+model.rowStart,line=document.createElement("tr");line.dataset.row=String(r);line.setAttribute("aria-rowindex",String(r+2));
      const number=document.createElement("td");number.textContent=String(r+1).padStart(2,"0");line.append(number);
      values.slice(viewport.start,viewport.end).forEach((text,index)=>{const c=index+viewport.start,cell=document.createElement("td");
        if(model.view==="chain"&&c>=2){cell.textContent=text;cell.title=text;line.append(cell);return;}
        const usage=model.view==="song"?model.tracks[c].slots[r]:null,selected=usage&&model.expandedSelection?.some(item=>item.track===model.tracks[c].id&&item.chain_use===usage.id),control=button((selected?"✓ ":"")+text,event=>{if(model.view==="song"&&usage&&(selectingChains||event.shiftKey))emit({kind:"toggle_expanded",track:model.tracks[c].id,chain_use:usage.id});else emit({kind:"focus",row:r,column:c});});
        control.dataset.value=(selected?"✓ ":"")+text;control.dataset.label=`${model.columns[c]}, row ${r+1}: ${text}`;
        control.dataset.gridRow=String(r);control.dataset.gridColumn=String(c);
        if(usage)cell.dataset.chainUse=`${model.tracks[c].id}:${usage.id}`;
        control.setAttribute("aria-label",control.dataset.label);control.setAttribute("aria-selected",String(r===model.row&&c===model.column));
        if(r===model.row&&c===model.column)control.className="focused";
        if(selected)control.classList.add("chain-selected");
        control.ondblclick=()=>{if(selectingChains&&model.view==="song")return;if(model.view==="chain"&&c===1){chainPlacementDialog(r);return;}model.view==="phrase"?noteDialog(r):request("confirm");};cell.append(control);line.append(cell);});body.append(line);});
    if(model.view==="chain"&&model.rowStart===0)for(let r=model.rows.length;r<12;r++){const line=document.createElement("tr");for(let c=0;c<=model.columns.length;c++){const cell=document.createElement("td");cell.textContent=c===0?String(r+1).padStart(2,"0"):"—";line.append(cell);}body.append(line);}
    table.append(body);gridNode.replaceChildren(table);gridNode.scrollLeft=oldLeft;gridNode.scrollTop=oldTop;
    updateGridFocus();
  }
  function updateGridFocus(){
    const key=`${model.view}:${model.row}:${model.column}`;
    if(key===gridFocusKey)return;gridFocusKey=key;
    const node=$("tracker-grid");
    for(const control of node.querySelectorAll("button[data-grid-row]")){
      const focused=Number(control.dataset.gridRow)===model.row&&Number(control.dataset.gridColumn)===model.column;
      control.classList.toggle("focused",focused);control.setAttribute("aria-selected",String(focused));
    }
    revealGridFocus(node.querySelector(".focused"));
  }
  function revealGridFocus(control){
    if(!control)return;
    const grid=$("tracker-grid"),header=grid.querySelector("thead"),laneLabel=grid.querySelector(".drum-grid th:first-child");
    // Native nearest scrolling does not account for sticky table overlays.
    control.style.scrollMarginTop=control.closest("thead")?"0px":`${header?.getBoundingClientRect().height||0}px`;
    control.style.scrollMarginLeft=`${laneLabel?.getBoundingClientRect().width||0}px`;
    control.scrollIntoView({block:"nearest",inline:"nearest"});
  }
  function setting(label,text,action){const row=document.createElement("div");row.className="setting-row";const name=document.createElement("span");name.textContent=label;row.append(name,button(text,action));return row;}
  const beatKey=position=>position.numerator+"/"+position.denominator;
  const sameOccurrence=(a,b)=>a&&b&&a.track===b.track&&a.chain_use===b.chain_use&&a.phrase_use===b.phrase_use;
  const locate=(span,view=model.view,row=null)=>emit({kind:"locate",occurrence:span.occurrence,at:span.start,row:row?.source_row??0,view,row_at:row?.at??null});
  let timelineFocusKey=null;
  const spanKey=span=>`${span.occurrence.track}:${span.occurrence.chain_use}:${span.occurrence.phrase_use}:${beatKey(span.start)}`;
  function selectedTimelineSpan(){
    return model.timeline.lanes.flatMap(lane=>lane.spans).find(span=>sameOccurrence(span.occurrence,model.occurrence)&&(!model.timelineFocus||compareBeats(span.start,model.timelineFocus)===0));
  }
  function renderTimeline(){
    const node=$("tracker-grid"),timeline=model.timeline;
    if(!timeline||timeline.error){node.textContent=timeline?.error||"No arrangement preview";$("tracker-timeline-source").replaceChildren();$("tracker-timeline-notice").textContent="";timelineRenderKey=null;return;}
    followMinimap();
    const key=stringify([model.view,timeline,model.expandedMissing]);
    if(key===timelineRenderKey){updateTimelineFocus();updateTimelinePlayback();return;}
    timelineRenderKey=key;timelineFocusKey=null;
    const oldLeft=node.scrollLeft,oldTop=node.scrollTop;
    node.dataset.mode=model.view;
    node.replaceChildren(model.view==="expanded"?expandedTimeline(timeline):minimapTimeline(timeline));
    const notice=$("tracker-timeline-notice"),notices=[...new Set(timeline.lanes.map(lane=>lane.notice).filter(Boolean))];
    notice.textContent=`BEATS ${beatLabel(timeline.start)}–${beatLabel(timeline.end)} · `+(model.view==="minimap"?"ACTIVITY / notes · BOUNDARIES / phrases · GREEN / playback · CYAN / focus":"All phrase rows align by exact quarter-note beats")+(notices.length?" · "+notices.join(" · "):"");
    if(model.view==="expanded"&&model.expandedMissing)notice.textContent+=` · ${model.expandedMissing} selected source(s) no longer exist. Restore with Undo or save a new View Group.`;
    node.scrollLeft=oldLeft;node.scrollTop=oldTop;updateTimelineFocus();updateTimelinePlayback();
  }
  function updateTimelineFocus(){
    const focused=selectedTimelineSpan(),selected=focused?spanKey(focused):null;
    const focusedAt=model.timelineRowAt??focused?.rows.find(row=>row.source_row===model.timelineRow)?.at;
    const key=stringify([selected,model.track,model.timelineRow,focusedAt,model.revision,model.sharedReferences]);
    if(key===timelineFocusKey)return;timelineFocusKey=key;
    for(const control of $("tracker-grid").querySelectorAll("[data-span-key]")){
      const active=control.dataset.spanKey===selected&&(control.dataset.sourceRow===undefined||Number(control.dataset.sourceRow)===model.timelineRow&&focusedAt&&control.dataset.rowAt===beatKey(focusedAt));
      control.classList.toggle("focused",active);if(control.tagName==="BUTTON")control.setAttribute("aria-selected",String(active));
    }
    for(const header of $("tracker-grid").querySelectorAll(".minimap-track"))header.classList.toggle("focused",Number(header.dataset.trackIndex)===model.track);
    revealGridFocus($("tracker-grid").querySelector("button.focused"));
    renderTimelineSource(focused);
  }
  function renderTimelineSource(focused){
    const source=$("tracker-timeline-source");source.replaceChildren();source.className="timeline-source is-"+model.view;
    if(!focused){source.textContent="Select an occurrence to open its source. Choose chains in Song to populate Expanded.";return;}
    const label=document.createElement("span"),detail=document.createElement("span"),references=model.sharedReferences?.references?.length||0;
    label.textContent=`T${String(model.track+1).padStart(2,"0")} / CHAIN ${model.selectedChain?.id} / PHRASE ${focused.phrase}`;
    detail.textContent=model.view==="expanded"?`ROW ${String((model.timelineRow||0)+1).padStart(2,"0")} · ${focused.name} · ${references} REFERENCES`:`BEATS ${beatLabel(focused.start)}–${beatLabel(focused.end)} · ${references} REFERENCES`;
    const sourceRow=focused.rows.find(row=>row.source_row===model.timelineRow&&(!model.timelineRowAt||compareBeats(row.at,model.timelineRowAt)===0));
    source.append(label,detail,button("OPEN CHAIN ›",()=>locate(focused,"chain")),button("EDIT PHRASE ›",()=>locate(focused,"phrase",sourceRow)));
    if(model.view==="minimap")source.append(button("EXPAND SELECTED ›",()=>emit({kind:"view",view:"expanded"})));
    const playback=document.createElement("span");playback.className="timeline-playback-label";source.append(playback);
  }
  function expandedLaneIndex(lane,times){
    const rows=new Map(),boundaries=new Map();
    for(const span of lane.spans){
      boundaries.set(beatKey(span.start),span);times.set(beatKey(span.start),span.start);
      indexExpandedRows(span,rows,times);
    }
    return {source:lane,rows,boundaries};
  }
  function indexExpandedRows(span,rows,times){
    for(const row of span.rows){const key=beatKey(row.at);rows.set(key,{row,span});times.set(key,row.at);}
  }
  function expandedTimeline(timeline){
    expandedPlaybackRows.clear();playingExpandedRows.clear();
    const table=document.createElement("table");table.className="tracker-table expanded";table.style.width=(68+timeline.lanes.length*452)+"px";
    const columns=document.createElement("colgroup"),axis=document.createElement("col");axis.style.width="64px";columns.append(axis);
    const head=document.createElement("thead"),header=document.createElement("tr"),time=document.createElement("th");time.textContent="TIME";header.append(time);
    const times=new Map(),lanes=[];
    for(const lane of timeline.lanes){appendExpandedColumns(columns);const title=document.createElement("th");title.colSpan=6;title.textContent=lane.name+(lane.muted?" · MUTED":"");header.append(title);lanes.push(expandedLaneIndex(lane,times));}
    head.append(header);table.append(columns,head);const body=document.createElement("tbody");
    for(const at of [...times.values()].sort(compareBeats)){
      if(lanes.some(lane=>lane.boundaries.has(beatKey(at))))body.append(expandedBoundary(lanes,at),expandedColumnHeaders(lanes));
      body.append(expandedEventRow(lanes,at));
    }
    table.append(body);return table;
  }
  function appendExpandedColumns(columns){
    for(const width of [72,96,40,76,76,76]){const column=document.createElement("col");column.style.width=width+"px";columns.append(column);}
  }
  function expandedBoundary(lanes,at){
    const line=document.createElement("tr");line.className="phrase-boundary";
    const time=document.createElement("td");time.textContent=beatLabel(at)+" BEAT";line.append(time);
    for(const lane of lanes){const cell=document.createElement("td");cell.colSpan=6;
      const starts=lane.boundaries.get(beatKey(at)),span=starts||lane.source.spans.find(span=>compareBeats(span.start,at)<0&&compareBeats(at,span.end)<0);
      if(span){const phrase=model.phrases[span.phrase],label=`PHRASE ${span.phrase} / ${starts?(phrase?.steps||"—")+" STEPS":"CONTINUED"} · REPEAT ${span.repeat+1}${span.generated?" · GENERATOR":""}${span.conditional?" · CONDITIONAL":""}`;
        const open=button(label,()=>locate(span));open.title=span.name;open.ondblclick=()=>locate(span,"phrase");cell.append(open);}
      line.append(cell);
    }
    return line;
  }
  function expandedColumnHeaders(lanes){
    const line=document.createElement("tr");line.className="expanded-columns";const time=document.createElement("td");time.textContent="BEAT";line.append(time);
    for(const lane of lanes)appendExpandedColumnHeaders(line);
    return line;
  }
  function appendExpandedColumnHeaders(line){
    for(const name of ["NOTE","INST","VEL","FX 1","FX 2","FX 3"]){const cell=document.createElement("td");cell.textContent=name;line.append(cell);}
  }
  function expandedEventRow(lanes,at){
    const line=document.createElement("tr"),time=document.createElement("td");time.textContent=beatLabel(at);line.append(time);
    for(const lane of lanes)appendExpandedEventCells(line,lane.rows.get(beatKey(at)),at);
    return line;
  }
  function appendExpandedEventCells(line,entry,at){
    const controls=[];
    for(let column=0;column<6;column++){const cell=document.createElement("td");
      if(entry){const control=button(entry.row.cells[column],()=>locate(entry.span,"expanded",entry.row));
        control.title=`${entry.span.name}, beat ${beatLabel(at)}, source row ${entry.row.source_row+1}`;control.setAttribute("aria-label",control.title+": "+entry.row.cells[column]);
        control.dataset.spanKey=spanKey(entry.span);control.dataset.sourceRow=String(entry.row.source_row);control.dataset.rowAt=beatKey(entry.row.at);
        control.dataset.label=control.getAttribute("aria-label");control.dataset.value=control.textContent;control.dataset.column=String(column);
        control.ondblclick=()=>locate(entry.span,"phrase",entry.row);cell.append(control);controls.push(control);
      }else{cell.textContent="—";cell.className="muted";}
      line.append(cell);
    }
    if(entry)expandedPlaybackRows.set(spanKey(entry.span)+":"+beatKey(entry.row.at),controls);
  }
  function minimapTimeline(timeline){
    const wrap=document.createElement("div");wrap.className="minimap";wrap.style.gridTemplateColumns=`48px repeat(${timeline.lanes.length},minmax(48px,1fr))`;
    const label=document.createElement("span");label.textContent="BAR";label.className="minimap-axis-title";wrap.append(label);
    for(const lane of timeline.lanes){const header=button(String(lane.index+1).padStart(2,"0"),()=>emit({kind:"focus",row:0,column:lane.index}));header.className="minimap-track";header.dataset.trackIndex=String(lane.index);header.title=lane.name+(lane.muted?" · muted":"");wrap.append(header);}
    const origin=value(timeline.start),duration=value(timeline.end)-origin,percent=at=>(value(at)-origin)/duration*100;
    wrap.append(minimapAxis(timeline,percent));
    for(const lane of timeline.lanes)wrap.append(minimapLane(lane,timeline,percent));
    return wrap;
  }
  function minimapAxis(timeline,percent){
    const axis=document.createElement("div");axis.className="minimap-axis";
    const stride=Math.max(1,Math.ceil(timeline.bars.length/8));
    timeline.bars.forEach((bar,index)=>{if(index%stride!==0)return;const mark=document.createElement("span");mark.textContent=String(bar.number).padStart(2,"0");mark.title=`Beat ${beatLabel(bar.at)} · ${bar.meter[0]}/${bar.meter[1]}`;mark.style.top=percent(bar.at)+"%";axis.append(mark);});
    return axis;
  }
  function minimapLane(lane,timeline,percent){
    const strip=document.createElement("div");strip.className="minimap-lane"+(lane.muted?" muted":"");strip.dataset.voiceColor=String(lane.index%3);
    for(const bar of timeline.bars){const line=document.createElement("span");line.className="minimap-bar";line.style.top=percent(bar.at)+"%";strip.append(line);}
    if(lane.conditional_from){const unknown=document.createElement("div");unknown.className="minimap-conditional";unknown.title=lane.notice;unknown.style.top=Math.max(0,percent(lane.conditional_from))+"%";strip.append(unknown);}
    for(const span of lane.spans)appendMinimapOccurrence(strip,lane,span,percent);
    const head=document.createElement("span");head.className="minimap-playhead";head.dataset.track=String(lane.track);strip.append(head);
    return strip;
  }
  function appendMinimapOccurrence(strip,lane,span,percent){
    const occurrence=button(span.generated?"G":"",()=>locate(span));occurrence.className="minimap-occurrence";occurrence.dataset.spanKey=spanKey(span);
    const top=Math.max(0,percent(span.start));occurrence.style.top=top+"%";occurrence.style.height=(Math.min(100,percent(span.end))-top)+"%";
    occurrence.setAttribute("aria-label",`${lane.name}, ${span.name}, beat ${beatLabel(span.start)}, repeat ${span.repeat+1}${span.conditional?", conditional timing":""}`);occurrence.title=occurrence.getAttribute("aria-label")+(span.generated?" · Generator recipe; open source for resolved preview":"");occurrence.ondblclick=()=>locate(span,"phrase");strip.append(occurrence);
    for(const activity of span.activity){const mark=document.createElement("span");mark.className="minimap-activity";mark.dataset.spanKey=spanKey(span);mark.style.top=percent(activity.at)+"%";mark.style.left=(8+(activity.pitch%12)*1.4)+"%";strip.append(mark);}
  }
  function updateTimelinePlayback(){
    const label=$("tracker-timeline-source").querySelector(".timeline-playback-label");
    if(label)label.textContent=model.playing&&model.playBeat?`PLAYING · BEAT ${value(model.playBeat).toFixed(2)}`:"";
    const heads=model.timelinePlayheads||[];
    if(model.view==="expanded"){
      const active=new Set(heads.filter(head=>head.rowAt&&model.playing).map(head=>spanKey(head)+":"+beatKey(head.rowAt)));
      for(const key of playingExpandedRows)if(!active.has(key))paintExpandedRow(key,false);
      for(const key of active)if(!playingExpandedRows.has(key))paintExpandedRow(key,true);
      playingExpandedRows=active;
    }
    for(const marker of $("tracker-grid").querySelectorAll(".minimap-playhead")){
      const head=heads.find(head=>String(head.occurrence.track)===marker.dataset.track);
      const at=head?value(head.at):null;
      const position=at===null?-1:(at-value(model.timeline.start))/(value(model.timeline.end)-value(model.timeline.start));
      marker.hidden=!model.playing||position<0||position>1;marker.style.top=(position*100)+"%";
      if(head)marker.title=`PHRASE ${head.phrase} · ROW ${head.row+1} · REPEAT ${head.phraseRepeat+1}${head.sourceTime?" · SOURCE POSITION":""}${head.conditional?" · CONDITIONAL TIMING":""}`;
    }
  }
  function paintExpandedRow(key,playing){
    for(const control of expandedPlaybackRows.get(key)||[]){
      control.parentElement.classList.toggle("playing",playing);
      control.textContent=(playing&&control.dataset.column==="0"?"▶ ":"")+control.dataset.value;
      control.setAttribute("aria-label",control.dataset.label+(playing?" · Playing":""));
    }
  }
  function shiftTimeline(direction){
    minimapFollow=false;const timeline=model.timeline;if(!timeline||timeline.error)return;
    const at=Math.max(0,value(timeline.start)+direction*(value(timeline.end)-value(timeline.start)));
    emit({kind:"preview_start",at:parseBeat(String(at))});
  }
  function timelinePositionDialog(){form("Timeline position",[{name:"at",label:"Quarter-note beat · snaps to containing bar",value:beatLabel(model.timeline?.start||beat(0))}],values=>{minimapFollow=false;emit({kind:"preview_start",at:parseBeat(values.at)});});}
  function followMinimap(){
    if(model.view!=="minimap"||!minimapFollow||!model.playing||!model.playBeat||!model.timeline||model.timeline.error)return;
    const timeline=model.timeline,at=value(model.playBeat);
    if(at>=value(timeline.start)&&at<value(timeline.end)){minimapPendingStart=null;return;}
    if(minimapPendingStart===beatKey(timeline.start))return;
    minimapPendingStart=beatKey(timeline.start);emit({kind:"preview_start",at:model.playBeat});
  }
  function renderSettings(){const node=$("tracker-settings");node.replaceChildren();
    node.append(setting("SONG NAME",model.song,()=>form("Song name",[{name:"name",label:"Name",value:model.song}],v=>emit({kind:"rename",name:v.name}))),setting("TEMPO",model.tempo+" BPM",()=>form("Tempo",[{name:"bpm",label:"Beats per minute",value:model.tempo,type:"number",min:0.001,max:4294967.295,step:0.001}],v=>emit({kind:"tempo",tempo:parseTempoMilliBpm(v.bpm)}))),setting("KEY / SCALE",model.key,scaleDialog),setting("INSTRUMENTS, MIXER & DEVICES","OPEN SYNTH",()=>emit({kind:"view",view:"instruments"})),setting("SAVE SONG","SAVE",()=>file("save")),setting("SAVE SONG AS","CHOOSE FILE",()=>file("save_as")),setting("OPEN SONG","CHOOSE FILE",()=>file("open")),setting("NEW SONG","EMPTY SONG",()=>file("new")));
    node.append(setting("MIDI INPUT & CONTROLLER BUTTONS","SYNTH SETTINGS",()=>request("synth_settings")),setting("PIANO ROLL FINE / RANGE","CONFIGURE BUTTONS",rollBindingsDialog));
    node.append(setting("EXTERNAL MIDI OUTPUT",(model.outputs?.connected?.length||0)+" CONNECTED",outputDialog));
    node.append(setting("SONG METER MAP",(model.meters||[]).map(change=>change.meter.join("/")+" @ "+beatLabel(change.at)).join(" · "),meterMapDialog));
    node.append(setting("MACHINE DEFAULTS & MIDI CLOCK",model.preferences?.status||"CONFIGURE",preferencesDialog));
    node.append(setting("MIDI CONTROL SURFACE",(model.preferences?.midi_controls?.length||0)+" MAPPINGS",midiControlsDialog));
    node.append(setting("SOUND LIBRARY","PUBLISH / ADOPT REVISION",soundLibraryDialog));
    if(model.recovery?.available)node.append(setting("RECOVER INTERRUPTED TAKE","REVIEW CHECKPOINT",recoveryDialog));
    if(model.phrase)node.append(setting("PHRASE SETTINGS",model.phrase.value.name,phraseDialog),setting("MAKE UNIQUE HERE","CREATE LOCAL COPY",makeUnique));
  }
  function meterMapDialog(){
    const meters=structuredClone(model.meters),revision=model.revision;
    form("Song meter map",[
      {name:"entry",label:"Meter change",value:0,options:meters.map((change,index)=>({value:index,label:change.meter.join("/")+" at beat "+beatLabel(change.at)}))},
      {name:"action",label:"Action",value:"edit",options:[{value:"edit",label:"Edit selected change"},{value:"add",label:"Add a meter change"},{value:"delete",label:"Delete selected change"}]}
    ],values=>{
      const index=Number(values.entry);
      if(values.action==="delete"){
        if(index===0)throw Error("The initial meter at beat 0 is required; edit it instead");
        meters.splice(index,1);emit({kind:"edit",revision,command:{kind:"set_meters",meters}});return;
      }
      queueMicrotask(()=>meterChangeDialog(meters,values.action==="add"?null:index,revision));
    });
    dialogNote("Positions use exact quarter-note beats. Each change must begin on a bar boundary under the preceding meter. Editing meter does not move notes.");
  }
  function meterChangeDialog(meters,index,revision){
    const change=index===null?{at:beat(4),meter:[4,4]}:meters[index];
    form(index===null?"Add meter change":"Edit meter change",[
      {name:"at",label:"Start at exact quarter-note beat (e.g. 0 or 15/2)",value:beatLabel(change.at)},
      {name:"numerator",label:"Beats per bar",value:change.meter[0],type:"number",min:1,max:65535},
      {name:"denominator",label:"Beat unit",value:change.meter[1],options:[1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384,32768].map(value=>({value,label:String(value)}))}
    ],values=>{
      const entry={at:parseBeat(values.at),meter:[Number(values.numerator),Number(values.denominator)]};
      if(index===null)meters.push(entry);else meters[index]=entry;
      meters.sort((a,b)=>compareBeats(a.at,b.at));
      emit({kind:"edit",revision,command:{kind:"set_meters",meters}});
    });
  }
  function toggleCurrentChain(){
    const track=model.tracks[model.track],usage=track?.slots[model.slot];
    if(!usage){chainSelectionDialog();return;}
    emit({kind:"toggle_expanded",track:track.id,chain_use:usage.id});
  }
  function chainSelectionDialog(){
    const entries=model.tracks.flatMap((track,index)=>track.slots.flatMap((usage,slot)=>usage?[{track:track.id,chain_use:usage.id,label:`${model.expandedSelection?.some(item=>item.track===track.id&&item.chain_use===usage.id)?"✓ ":""}Track ${index+1} · slot ${slot+1} · ${model.chains[usage.chain].name}`}]:[]));
    form("Choose chain occurrences",[
      {name:"entry",label:"Occurrence",value:0,options:entries.length?entries.map((entry,index)=>({value:index,label:entry.label})):[{value:"",label:"No chain occurrences yet"}]},
      {name:"action",label:"Action",value:"toggle",options:[{value:"toggle",label:"Add / remove selected occurrence"},{value:"clear",label:"Clear explicit selection"}]}
    ],values=>{
      if(values.action==="clear"){request("clear_expanded");return;}
      const entry=entries[Number(values.entry)];if(!entry)throw Error("Create a chain in Song first");
      emit({kind:"toggle_expanded",track:entry.track,chain_use:entry.chain_use});
    });
    dialogNote("Choose up to 16 occurrences, including several from one track. With no explicit selection, Expanded shows the current chain.");
  }
  function viewGroupsDialog(){
    const entries=Object.entries(model.viewGroups||{});
    form("Saved View Groups",[
      {name:"group",label:"Group",value:entries[0]?.[0]??"",options:entries.length?entries.map(([name,group])=>({value:name,label:`${name} · ${group.selections.length} sources${group.missing?" · "+group.missing+" missing":""}`})):[{value:"",label:"No saved groups"}]},
      {name:"action",label:"Action",value:entries.length?"recall":"save",options:[...(entries.length?[{value:"recall",label:"Recall in Expanded"},{value:"delete",label:"Delete saved group"}]:[]),{value:"save",label:"Save current selection as a group"}]}
    ],values=>{
      if(values.action==="save"){queueMicrotask(saveViewGroupDialog);return;}
      if(values.action==="delete")edit({kind:"delete_view_group",name:values.group});
      else emit({kind:"recall_view_group",name:values.group});
    });
    dialogNote("Groups are saved with the Song. Deleted sources remain marked missing; recall never substitutes a different chain. Save and delete support Undo.");
  }
  function saveViewGroupDialog(){
    const selections=structuredClone(model.viewSelection||[]);
    form("Save View Group",[{name:"name",label:"Name · an existing name replaces that group",value:"My view"}],values=>{
      if(!selections.length)throw Error("Select at least one chain occurrence in Song first");
      edit({kind:"put_view_group",name:values.name.trim(),selections});
    });
    dialogNote(selections.length+" selected source(s) will be saved.");
  }
  function helpDialog(){
    form("Tracker help",[{name:"action",label:"Next action",value:"close",options:[{value:"close",label:"Return to Tracker"},{value:"synth",label:"Open Synth for controller assignments"}]}],values=>{if(values.action==="synth")request("synth_settings");});
    dialogNote("Drum: Left / Right selects a step; Up / Down selects a pitch/output lane. Confirm toggles that exact hit; Clear removes only its notes and preserves controls/FX. Edit + Left / Right changes hit velocity; Edit + Up opens hit or empty-lane settings. Add Drum Lane creates a transient lane; its notes are saved with the Song.");
    dialogNote("Keyboard: W/A/S/D navigate; K + direction edits; Enter confirms/inserts; Q goes back; E clears; Space plays/stops; [ / ] undo/redo; T opens Main menu; Shift + Q opens Main menu from Song; 1 or 3 opens Song; 2 opens Synth. Shift + W opens Phrase Settings in a phrase, otherwise Tracker Settings. Shift + S opens Mixer from Song, Quick Jump elsewhere; Shift + A Back, Shift + D Open.");
    dialogNote("Controller roles: directions navigate; Edit + direction adjusts; tapping Edit or Next Patch confirms; Previous Patch goes back; Preview plays/stops; Settings opens Tracker Settings. Shift + Back opens Main menu from Song. Hold Shift across page changes. Above the first grid row, Up focuses the visible toolbar; Back returns to the grid. In dialogs: Up/Down moves focus, Left/Right changes a value, Edit or Next Patch confirms, Previous Patch cancels. Confirm a text field to open the character picker.");
    dialogNote("Song selection: navigate to a chain, then choose SELECT CURRENT CHAIN in More Actions; repeat to toggle it off. CHOOSE CHAINS also reaches every occurrence. VIEW GROUPS saves and recalls selections.");
    dialogNote("Song File > LOAD EXAMPLE opens Window Light with five native sounds, saved mix and expressive notes. Unsaved work gets Save / Discard / Cancel before replacement. NEW SONG starts empty.");
    dialogNote("Piano Roll: CHOOSE NOTE / EVENT lists exact event IDs, including overlaps, with select/toggle/edit/raw-expression actions. PREVIOUS/NEXT NOTE selects one note. INSERT NOTE always adds a distinct event. SELECTION offers copy/cut/paste and duration edits. RECORD SETUP, RECORD and END TAKE stay explicit in More Actions.");
    dialogNote("Physical button assignments use Synth’s existing Settings > CONTROLLER BUTTONS page, including capture, conflict feedback and reset. Tracker Settings > MIDI INPUT & CONTROLLER BUTTONS opens Synth Settings. Piano Roll Fine/Range bindings live in Tracker Settings; defaults are right shoulder and Select/Back. Edit/Shift assignments take priority on conflicts. Left stick/D-pad moves cursor; Range extends selection. Edit+Left/Right nudges selected notes. Right stick Left/Right resizes; Up/Down changes selected Pitch/Velocity field. Fine uses 1/16 edit grid. Edit+Up opens context actions.");
  }
  function recoveryDialog(){
    const recovery=model.recovery;
    form("Review interrupted take",[{name:"action",label:"Action",value:"recover",options:[{value:"recover",label:"Restore checkpoint baseline and draft for review"},{value:"discard",label:"Discard this checkpoint"}]}],values=>emit({kind:"recovery",action:values.action}));
    dialogNote("Checkpoint time: "+(recovery.checkpointUnixMillis?new Date(recovery.checkpointUnixMillis).toLocaleString():"Unavailable in this older checkpoint"));
    dialogNote("Current Song: "+(recovery.currentSong??model.song)+". Checkpoint baseline: "+(recovery.baselineSong??"Unavailable")+". Recovered draft: "+(recovery.takeEvents??0)+" events.");
    dialogNote("Recovery restores the checkpoint’s Song and sound baseline, then exposes its interrupted take for review and explicit commit. The original Song file remains unchanged. Discard removes the recovery checkpoint.");
  }
  function form(title,fields,submit,keepOpen=false){
    chordDraftPreview=null;pendingEditPreview=null;activeChordLibrary=false;
    leaveControls();delete $("tracker-dialog").dataset.songFile;
    closeCharacterChooser(false);clearControllerFocus();
    formRevision=model.revision;
    for(const control of $("tracker-form").querySelectorAll("button"))control.disabled=false;
    $("tracker-form").setAttribute("aria-busy","false");
    $("tracker-dialog").classList.remove("piano-sheet","piano-setup");$("tracker-form").querySelector("footer").hidden=false;
    $("tracker-form").querySelector('button[type="submit"]').textContent="APPLY";
    $("tracker-dialog-title").textContent=title;$("tracker-form-error").textContent="";const container=$("tracker-fields");container.replaceChildren();
    fields.forEach(field=>{const label=document.createElement("label");label.textContent=field.label;if(field.wide)label.className="wide";
      const input=document.createElement(field.options?"select":"input");input.name=field.name;
      if(field.options)field.options.forEach(option=>{const node=document.createElement("option");node.value=String(option.value);node.textContent=option.label;input.append(node);});
      else{input.type=field.type||"text";if(field.min!==undefined)input.min=field.min;if(field.max!==undefined)input.max=field.max;if(field.step!==undefined)input.step=field.step;input.required=field.required!==false;}
      input.value=String(field.value??"");label.append(input);container.append(label);});
    const hint=document.createElement("p");hint.className="controller-hint";hint.style.cssText="grid-column:1/-1;color:var(--color-text-muted);font-size:11px";hint.textContent="Controller: Up / Down focus · Left / Right change · Confirm edit / apply · Back cancel";container.append(hint);
    $("tracker-form").onsubmit=event=>{event.preventDefault();try{submit(Object.fromEntries(new FormData(event.target)));if(!keepOpen)closeDialog();}catch(error){$("tracker-form-error").textContent=error.message;}};
    if(!$("tracker-dialog").open){$("tracker-dialog").showModal();emit({kind:"text_entry",active:true});}container.querySelector("input,select")?.focus();
    renderIdentity();
  }
  function closeDialog(){cancelSharedEdit();cancelFileDialog();hideDialog();}
  function cancelSharedEdit(){
    if(activeSharedToken===null)return;
    const token=activeSharedToken;activeSharedToken=null;
    emit({kind:"shared_decision",token,choice:null,remember:false});
  }
  function renderSharedEdit(pending){
    if(!pending){if(activeSharedToken!==null){activeSharedToken=null;hideDialog();}return;}
    if(pending.token<=lastSharedToken)return;
    lastSharedToken=pending.token;activeSharedToken=pending.token;
    form("Shared "+pending.definition.kind+" · "+pending.references.length+" uses",[
      {name:"choice",label:"Where should this edit apply?",value:pending.canMakeUnique?"unique":"shared",options:[...(pending.canMakeUnique?[{value:"unique",label:"Make Unique · only this use"}]:[]),{value:"shared",label:"Edit Shared · all uses"}]},
      {name:"remember",label:"Remember for this session",value:"false",options:[{value:"false",label:"Ask each time"},{value:"true",label:"Remember choice"}]}
    ],values=>{activeSharedToken=null;emit({kind:"shared_decision",token:pending.token,choice:values.choice,remember:values.remember==="true"});});
    dialogNote(pending.references.map(referenceLabel).join(" · "));
    if(pending.recordingRestricted)dialogNote("Other shared phrases can be edited together during a take. Make Unique changes the arrangement; finish the take first.");
    else if(!pending.canMakeUnique)dialogNote("This edit also changes the arrangement. Apply it to all uses, or cancel and edit the phrase separately to make it unique.");
  }
  function referenceLabel(reference){return `${reference.track_name} · slot ${reference.slot+1}`+(reference.phrase_row===null?"":` · phrase ${reference.phrase_row+1}`);}
  function sharedReferencesDialog(){
    const references=model.sharedReferences?.references||[];
    form("Shared References",[{name:"reference",label:"Arrangement uses",value:0,options:references.map((reference,index)=>({value:index,label:referenceLabel(reference)}))},
      {name:"reset",label:"Shared edit preference",value:"false",options:[{value:"false",label:model.sharedChoice?"Remembered: "+model.sharedChoice:"Ask each time"},{value:"true",label:"Reset · ask each time"}]}],values=>{if(values.reset==="true")request("reset_shared_choice");});
    dialogNote("Repeated playback of a slot uses the same definition. Make Unique copies only the selected use, including its parent chain when needed.");
  }
  function hideDialog(){closeCharacterChooser(false);clearControllerFocus();chordDraftPreview=null;pendingEditPreview=null;activeChordLibrary=false;formRevision=null;$("tracker-dialog").close();renderIdentity();emit({kind:"text_entry",active:false});}
  function cancelFileDialog(){
    if(!activeFileDialog)return;
    const token=activeFileDialog.token;activeFileDialog=null;retiredFileDialogs.add(token);
    clearFileForm();
    emit({kind:"file_cancel",token});
  }
  function clearFileForm(){closeCharacterChooser(false);clearControllerFocus();$("tracker-fields").replaceChildren();$("tracker-form-error").textContent="";$("tracker-form").onsubmit=null;}
  function fileInputs(){return [...$("tracker-fields").querySelectorAll("input,select")];}
  function fileInput(name){return fileInputs().find(input=>input.name===name);}
  function fileValues(){return Object.fromEntries(fileInputs().map(input=>[input.name,input.value]));}
  function fileFocusKey(){const node=document.activeElement;return node?.name||node?.dataset.fileAction||null;}
  function restoreFileFocus(key){
    const node=[...$("tracker-form").querySelectorAll("input,select,button")].find(node=>key&&(node.name===key||node.dataset.fileAction===key)&&!node.disabled);
    if(node)focusController(node);
  }
  function fileBusy(busy,message){
    if(!activeFileDialog)return;
    if(busy)activeFileDialog.focus=fileFocusKey()||activeFileDialog.focus;
    activeFileDialog.waiting=busy;
    for(const node of $("tracker-form").querySelectorAll("input,select,button")){
      if(node===$("tracker-dialog-close")||node===$("tracker-dialog-cancel"))continue;
      node.disabled=busy||node.dataset.fileUnavailable==="true";
    }
    if(message)activeFileDialog.status.textContent=message;
    $("tracker-form").setAttribute("aria-busy",String(busy));
  }
  function fileAction(request){
    if(!activeFileDialog||activeFileDialog.waiting||request.token!==activeFileDialog.token)return;
    $("tracker-form-error").textContent="";
    fileBusy(true,"Working…");emit(request);
  }
  function renderFileDialog(projection){
    if(!projection){
      if(activeFileDialog){retiredFileDialogs.add(activeFileDialog.token);activeFileDialog=null;hideDialog();clearFileForm();}
      return;
    }
    if(retiredFileDialogs.has(projection.token))return;
    if(activeFileDialog?.token===projection.token&&activeFileDialog.version>=projection.version)return;
    const previous=activeFileDialog,continuing=previous?.token===projection.token;
    const values=continuing?fileValues():{},focus=continuing?fileFocusKey()||previous.focus:null;
    if(previous&&!continuing)retiredFileDialogs.add(previous.token);
    activeFileDialog={token:projection.token,version:projection.version,projection,waiting:false,status:null,focus};
    if(projection.mode==="confirm")renderFileConfirmation(projection);
    else if(projection.overwrite)renderFileOverwrite(projection);
    else renderFileBrowser(projection,continuing&&previous.projection.directory===projection.directory?values:{});
    $("tracker-form-error").textContent=projection.error||"";
    fileBusy(!!projection.loading,projection.loading?"Loading…":projection.error?"Action failed. Review the error and retry.":"Ready");
    restoreFileFocus(focus);
  }
  function renderFileConfirmation(projection){
    form(projection.title||"Confirm file action",[{name:"decision",label:projection.message||"Choose an action",value:projection.choices?.[0]?.value,options:projection.choices||[]}],values=>{
      if(!(projection.choices||[]).some(choice=>choice.value===values.decision))throw Error("Choose an available action");
      fileAction({kind:"file_decision",token:projection.token,decision:values.decision});
    },true);
    activeFileDialog.status=dialogNote(projection.message||"Choose an action");
  }
  function renderFileOverwrite(projection){
    const path=projection.overwrite;
    form(projection.title||"Replace existing file",[{name:"overwrite",label:"This destination already exists",value:"return",options:[{value:"return",label:"Return to file browser"},{value:"replace",label:"Replace existing destination"}]}],values=>{
      if(values.overwrite==="replace")fileAction({kind:"file_choose",token:projection.token,path,overwrite:true});
      else fileAction({kind:"file_browse",token:projection.token,directory:projection.directory});
    },true);
    dialogNote("Replace this exact destination: "+path);
    activeFileDialog.status=dialogNote("Replacement requires an explicit choice.");
  }
  function renderFileBrowser(projection,draft){
    const entries=projection.entries||[],selected=entries.some(entry=>entry.path===draft.entry)?draft.entry:"";
    form(projection.title||"Choose a file",[
      {name:"entry",label:"Directory entries · choose one, then Open selected",value:selected,options:[{value:"",label:entries.length?"Choose an entry":"This directory is empty"},...entries.map(entry=>({value:entry.path,label:(entry.directory?"Folder · ":"File · ")+entry.name}))]},
      {name:"directory",label:"Folder path · edit, then Browse path",value:draft.directory??projection.directory},
      {name:"filename",label:projection.mode==="new_folder"?"New folder name":projection.mode==="save"?"Save filename":"Filename or absolute file path",value:draft.filename??projection.filename??"",required:false}
    ],values=>chooseFilePath(projection,values),true);
    const entry=fileInput("entry"),filename=fileInput("filename");
    entry.onchange=()=>{const selected=entries.find(item=>item.path===entry.value);if(selected)filename.value=selected.directory?"":selected.name;};
    const actions=document.createElement("div");actions.className="wide";actions.style.cssText="grid-column:1/-1;display:flex;gap:8px;flex-wrap:wrap";
    const action=(label,key,run)=>{const node=button(label,()=>{try{run();}catch(error){$("tracker-form-error").textContent=error.message;}});node.dataset.fileAction=key;actions.append(node);return node;};
    action("OPEN SELECTED","selected",()=>{const selected=entries.find(item=>item.path===fileInput("entry").value);if(!selected)throw Error("Choose a directory entry first");if(selected.directory)fileAction({kind:"file_browse",token:projection.token,directory:selected.path});else{filename.value=selected.name;chooseFilePath(projection,{...fileValues(),directory:projection.directory});}});
    action("BROWSE PATH","browse",()=>{const directory=fileInput("directory").value;if(!directory)throw Error("Enter a folder path");fileAction({kind:"file_browse",token:projection.token,directory});});
    const parent=action("PARENT","parent",()=>fileAction({kind:"file_browse",token:projection.token,directory:projection.parent}));parent.dataset.fileUnavailable=String(!projection.parent);
    $("tracker-fields").append(actions);
    dialogNote(projection.mode==="new_folder"?"Enter a new folder name, then Apply. Open selected browses an existing folder.":"Open selected enters a folder or chooses a file. Apply uses the filename and folder path above.");
    activeFileDialog.status=dialogNote("");
  }
  function chooseFilePath(projection,values){
    if(!activeFileDialog||activeFileDialog.waiting)return;
    const selected=(projection.entries||[]).find(entry=>entry.path===values.entry);
    if(!values.filename&&selected?.directory){fileAction({kind:"file_browse",token:projection.token,directory:selected.path});return;}
    if(!values.filename)throw Error(projection.mode==="new_folder"?"Enter a new folder name":"Choose a file or enter a filename");
    if(!values.directory&&!values.filename.startsWith("/"))throw Error("Enter a folder path");
    const path=selected&&!selected.directory&&values.directory===projection.directory&&values.filename===selected.name?selected.path:values.filename.startsWith("/")?values.filename:values.directory.replace(/\/+$/,"")+"/"+values.filename;
    fileAction({kind:"file_choose",token:projection.token,path,overwrite:false});
  }
  // Projections may repeat before an acknowledgement returns. Mark each input as
  // queued first, and yield between actions so a submitted menu can open its form.
  function consumeUiInputs(inputs){
    for(const input of inputs){if(!Number.isSafeInteger(input.sequence)||input.sequence<=lastUiSequence)continue;lastUiSequence=input.sequence;pendingUiInputs.push(input);}
    if(!handlingUiInputs)void runUiInputs();
  }
  async function runUiInputs(){
    handlingUiInputs=true;
    while(pendingUiInputs.length){const input=pendingUiInputs.shift();try{controllerAction(input.action);}catch(error){$("tracker-form-error").textContent=error.message;}finally{emit({kind:"ui_ack",sequence:input.sequence});}await Promise.resolve();}
    handlingUiInputs=false;
  }
  function surfaceControls(){
    const ids=model.view==="settings"?["tracker-settings"]:["tracker-actions","tracker-grid-window","tracker-timeline-source"];
    const roots=ids.map($).filter(Boolean);
    const controls=roots.flatMap(root=>[...root.querySelectorAll("button,summary")]);
    return controls.filter(node=>!node.disabled&&!node.hidden&&node.getClientRects().length);
  }
  function focusControls(){
    controllerSurface={view:model.view,index:0};
    emit({kind:"controls_focus",active:true});
    focusController(surfaceControls()[0]);
  }
  function leaveControls(){
    if(!controllerSurface)return;
    controllerSurface=null;clearControllerFocus();
    emit({kind:"controls_focus",active:false});
  }
  function restoreSurfaceFocus(){
    if(!controllerSurface)return;
    if(controllerSurface.view!==model.view||$("tracker-dialog").open||(model.view!=="settings"&&!model.controlsFocused)){leaveControls();return;}
    const controls=surfaceControls();
    const index=controls.indexOf(document.activeElement);
    if(index>=0)controllerSurface.index=index;
    else focusController(controls[Math.min(controllerSurface.index,controls.length-1)]);
  }
  function surfaceAction(action){
    if(!controllerSurface&&model.view==="settings")controllerSurface={view:model.view,index:0};
    const controls=surfaceControls();if(!controls.length)return;
    let index=controls.indexOf(document.activeElement);
    if(action==="previous"||action==="decrease")index=index<0?controls.length-1:(index+controls.length-1)%controls.length;
    else if(action==="next"||action==="increase")index=index<0?0:(index+1)%controls.length;
    else index=Math.max(0,index);
    const node=controls[index];
    if(controllerSurface)controllerSurface.index=index;
    focusController(node);
    if(action!=="confirm")return;
    if(node.tagName==="SUMMARY"){node.click();return;}
    leaveControls();node.click();
  }
  function clearControllerFocus(){
    if(!controllerFocus)return;
    controllerFocus.node.style.outline=controllerFocus.outline;controllerFocus.node.style.outlineOffset=controllerFocus.outlineOffset;controllerFocus=null;
  }
  function focusController(node){
    clearControllerFocus();if(!node)return;
    controllerFocus={node,outline:node.style.outline,outlineOffset:node.style.outlineOffset};node.style.outline="3px solid var(--color-accent-focus)";node.style.outlineOffset="-3px";node.focus();node.scrollIntoView({block:"nearest",inline:"nearest"});
  }
  function controllerFields(){
    const root=controllerText?controllerText.panel:$("tracker-fields");
    const fields=[...root.querySelectorAll("input,select,textarea,button")];
    if(!controllerText)fields.push(...$("tracker-form").querySelectorAll('footer button[type="submit"]'),...$("tracker-form").querySelectorAll('header button,footer button:not([type="submit"])'));
    return fields.filter(node=>!node.disabled&&!node.hidden&&node.type!=="hidden"&&node.getClientRects().length);
  }
  function moveControllerFocus(direction){
    const fields=controllerFields();if(!fields.length)return;
    const current=fields.indexOf(document.activeElement),next=current<0?(direction<0?fields.length-1:0):(current+direction+fields.length)%fields.length;focusController(fields[next]);
  }
  function changeControllerValue(node,direction){
    if(node.tagName==="SELECT"){
      const options=[...node.options].filter(option=>!option.disabled),current=options.indexOf(node.selectedOptions[0]);if(!options.length)return;
      node.value=options[(Math.max(0,current)+direction+options.length)%options.length].value;
    }else if(node.tagName==="INPUT"&&node.type==="number")node.stepUp(direction);
    else return;
    node.dispatchEvent(new Event("input",{bubbles:true}));node.dispatchEvent(new Event("change",{bubbles:true}));
  }
  function controllerAction(action){
    const dialog=$("tracker-dialog");
    if(action==="quick_jump"){if(!dialog.open)quickJump();return;}
    if(action==="phrase_settings"){if(!dialog.open)phraseDialog();return;}
    if(action==="context_edit"){if(!dialog.open)contextEdit();return;}
    if(action==="menu"){if(!dialog.open)menu();return;}
    if(action==="focus_controls"){if(!dialog.open)focusControls();return;}
    if(action==="shift_up"&&dialog.dataset.songFile==="true"){hideDialog();emit({kind:"view",view:"settings"});return;}
    if(action==="back"){if(controllerText)closeCharacterChooser(true);else if(dialog.open)closeDialog();else if(controllerSurface)leaveControls();else request("back");return;}
    if(!dialog.open){if(controllerSurface||model.view==="settings")surfaceAction(action);return;}
    if(action==="edit_previous"||action==="edit_next"){
      const node=$("tracker-fields").querySelector("[data-controller-chord]")||document.activeElement;
      if(node){focusController(node);changeControllerValue(node,action==="edit_previous"?-1:1);}return;
    }
    if(action==="previous"||action==="next"){
      const delta=action==="previous"?-1:1,node=document.activeElement;
      if(node?.dataset.controllerBrowse==="true")changeControllerValue(node,delta);
      else moveControllerFocus(delta);return;
    }
    const fields=controllerFields();
    // Disabling a submitted file action can blur its button. A repeated Confirm
    // must not fall through to Cancel while the backend is still working.
    if(activeFileDialog?.waiting&&action==="confirm"&&!fields.includes(document.activeElement))return;
    const node=fields.includes(document.activeElement)?document.activeElement:fields[0];if(!node)return;focusController(node);
    if(action==="decrease"||action==="increase"){changeControllerValue(node,action==="decrease"?-1:1);return;}
    if(action!=="confirm")return;
    if(controllerText&&node===controllerText.characters){insertControllerCharacter();return;}
    if(node.tagName==="BUTTON"){node.click();return;}
    if(node.tagName==="INPUT"&&node.type!=="number"||node.tagName==="TEXTAREA"){openCharacterChooser(node);return;}
    moveControllerFocus(1);
  }
  function openCharacterChooser(input){
    const panel=document.createElement("section");panel.className="controller-character-chooser";panel.style.cssText="grid-column:1/-1;display:flex;gap:12px;flex-wrap:wrap;align-items:end;padding:16px;border:1px solid var(--color-accent-focus)";
    const label=document.createElement("label");label.textContent="Character · Left / Right chooses, Confirm inserts";const characters=document.createElement("select");characters.setAttribute("aria-label","Character to insert");
    for(const character of "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 -_./,:;#@!?+*=()[]'\""){const option=document.createElement("option");option.value=character;option.textContent=character===" "?"SPACE":character;characters.append(option);}label.append(characters);panel.append(label);
    const start=input.selectionStart??input.value.length,end=input.selectionEnd??start;
    controllerText={input,original:input.value,start,end,panel,characters};
    panel.append(button("DELETE",deleteControllerCharacter),button("DONE",()=>{closeCharacterChooser(false);moveControllerFocus(1);}),button("CANCEL TEXT",()=>closeCharacterChooser(true)));
    $("tracker-fields").append(panel);focusController(characters);
  }
  function replaceControllerText(insert,removePrevious=false){
    const state=controllerText;if(!state)return;
    let start=state.start;
    if(removePrevious&&start===state.end){const prefix=Array.from(state.input.value.slice(0,start));prefix.pop();start=prefix.join("").length;}
    state.input.value=state.input.value.slice(0,start)+insert+state.input.value.slice(state.end);state.start=state.end=start+insert.length;
    state.input.setSelectionRange(state.start,state.end);state.input.dispatchEvent(new Event("input",{bubbles:true}));
  }
  function insertControllerCharacter(){replaceControllerText(controllerText.characters.value);}
  function deleteControllerCharacter(){replaceControllerText("",true);}
  function closeCharacterChooser(cancel){
    if(!controllerText)return;
    const state=controllerText;controllerText=null;clearControllerFocus();state.panel.remove();if(cancel){state.input.value=state.original;state.input.dispatchEvent(new Event("input",{bubbles:true}));}focusController(state.input);
  }
  const destinations=[{value:"song",label:"Tracker · Song"},{value:"chain",label:"Tracker · Chain"},{value:"phrase",label:"Tracker · Phrase"},{value:"expanded",label:"Expanded"},{value:"minimap",label:"Minimap"},{value:"instruments",label:"Instruments"},{value:"chords",label:"Chord Bank"},{value:"mixer",label:"Mixer"},{value:"sends",label:"Sends"},{value:"settings",label:"Tracker Settings"},{value:"devices",label:"Synth & Device Settings"}];
  function goTo(destination){
    if(destination==="chords"){queueMicrotask(chordBankDialog);return;}
    if(["instruments","mixer","sends","devices"].includes(destination)){emit({kind:"synth_page",page:destination==="devices"?"settings":destination});return;}
    emit({kind:"view",view:destination});
  }
  function transportDialog(){
    const phrase=model.phrase?.value,scope=model.view==="phrase"?"phrase":model.view==="chain"?"chain":"song";
    const at=scope==="phrase"&&phrase?phraseEditBeat(phrase):model.timelineFocus||model.playBeat||beat(0);
    form("Play from position / audition",[
      {name:"scope",label:"Playback scope",value:scope,options:[{value:"song",label:"Whole Song"},{value:"chain",label:"Selected Chain only"},{value:"phrase",label:"Selected Phrase only"}]},
      {name:"at",label:"Start · quarter-note beats from scope beginning",value:beatLabel(at)}
    ],values=>emit({kind:"start_at",scope:values.scope,at:parseBeat(values.at)}));
    dialogNote("Restarts transport at this exact position. Earlier notes are released, not retriggered. Interrupted recording stays recoverable. External clock owns its own seek position.");
  }
  function navigationMenu(title,entries){
    form(title,[],()=>{},true);
    $("tracker-form").querySelector("footer").hidden=true;
    const container=$("tracker-fields");container.replaceChildren();
    for(const entry of entries)container.append(button(entry.label,()=>{hideDialog();queueMicrotask(entry.run);}));
    focusController(container.querySelector("button"));
  }
  function quickJump(){navigationMenu("Quick Jump",[...destinations.map(destination=>({label:destination.label,run:()=>goTo(destination.value)})),{label:"Song File",run:songFileMenu}]);}
  function menu(){navigationMenu("Main menu",[
    {label:"Tracker",run:()=>goTo("song")},{label:"Instruments",run:()=>goTo("instruments")},
    {label:"Chord Bank",run:chordBankDialog},{label:"Mixer",run:()=>goTo("mixer")},
    {label:"Sends",run:()=>goTo("sends")},{label:"Settings",run:()=>goTo("settings")}
  ]);}
  function songFileMenu(){
    navigationMenu("Song File",[
      ...["save","save_as","open","new","import_synth","export_synth","collect_assets"].map(action=>({label:action.replaceAll("_"," ").toUpperCase(),run:()=>file(action)})),
      {label:"LOAD EXAMPLE",run:()=>request("example")}
    ]);
    $("tracker-dialog").dataset.songFile="true";
    dialogNote("Shift + Up opens current Song settings. Back returns without changing the Song.");
  }
  function contextEdit(){
    if(!model.phrase)return;
    if(model.phrase.value.form.kind==="piano_roll"){pianoEditNotes();return;}
    if(model.phrase.value.form.kind==="drum"){
      const lane=drumLanes()[model.column];if(!lane)return;
      const event=lane.events.find(event=>compareBeats(event.at,gridBeat(model.phrase.value,model.row))===0);
      if(event)noteDialog(model.row,event);else drumLaneDialog(lane);
      return;
    }
    if(model.column>=3){
      const event=model.phrase.value.events.find(event=>compareBeats(event.at,phraseEditBeat())===0);
      if(event?.fx[model.column-3]?.kind==="chord_revision")chordBankDialog();
      else fxDialog(model.column-3);
      return;
    }
    if(model.column===1){targetDialog();return;}
    noteDialog();
  }
  function phraseEventDraft(){
    const at=phraseEditBeat(),existing=model.phrase.value.events.find(event=>compareBeats(event.at,at)===0);
    return existing?structuredClone(existing):{id:model.nextIdentity,at,target:chosenTarget(soundTargets()[0]?.value,1),note:null,controls:[],fx:[null,null,null]};
  }
  function targetDialog(){
    const source={phrase:model.phrase.id,revision:model.revision},event=phraseEventDraft();
    form("Instrument / MIDI channel",[
      {name:"target",label:"Destination",value:targetChoice(event.target),options:soundTargets()},
      {name:"channel",label:"MIDI channel",value:event.target.channel,type:"number",min:1,max:16}
    ],values=>{event.target=chosenTarget(values.target,values.channel);emit({kind:"edit",revision:source.revision,command:{kind:"put_event",phrase:source.phrase,event}});});
    focusController($("tracker-fields").querySelector("select"));
  }
  function fxDialog(slot){
    const source={phrase:model.phrase.id,revision:model.revision},event=phraseEventDraft(),original=event.fx[slot];
    form("FX "+(slot+1),[
      {name:"command",label:"Command · Up / Down browse",value:fxChoice(original),options:fxTypes.map(value=>({value,label:value.toUpperCase()}))},
      {name:"value",label:fxValueLabel(fxChoice(original)),value:fxValue(original),required:false}
    ],values=>{
      event.fx[slot]=values.command===fxChoice(original)&&values.value===fxValue(original)?original:parseFx(values.command,values.value);
      emit({kind:"edit",revision:source.revision,command:{kind:"put_event",phrase:source.phrase,event}});
    });
    const command=$("tracker-fields").querySelector("select"),value=$("tracker-fields").querySelector("input");
    command.dataset.controllerBrowse="true";
    command.onchange=()=>{value.parentElement.firstChild.textContent=fxValueLabel(command.value);value.disabled=command.value==="none";};
    command.onchange();focusController(command);
  }
  function pianoControlsDialog(){form("Piano Roll edit controls",[
    {name:"grid",label:"Edit grid · exact quarter-note beats (e.g. 1/4 or 1/6)",value:beatLabel(model.pianoControls.grid)},
    {name:"value",label:"Right stick Up / Down",value:model.pianoControls.value,options:[{value:"pitch",label:"Pitch · semitones"},{value:"velocity",label:"Attack velocity"},{value:"release_velocity",label:"Release velocity"}]}
  ],values=>{const grid=parseBeat(values.grid);if(!grid.numerator)throw Error("Edit grid must be positive");emit({kind:"piano_control",control:{kind:"grid",grid}});emit({kind:"piano_control",control:{kind:"value",value:values.value}});});dialogNote("Edit grid changes navigation and snapping only. Phrase division and recorded timing stay unchanged. Fine uses 1/16 of this grid.");}
  function rollBindingsDialog(){
    const bindings=model.preferences?.roll_bindings,options=model.preferences?.roll_buttons;if(!bindings||!options)throw Error("Wait for machine preferences to load");
    form("Piano Roll controller modifiers",[{name:"fine",label:"Fine · 1/16 edit grid",value:bindings.fine,options},{name:"range",label:"Range · rectangular selection",value:bindings.range,options}],values=>{if(values.fine===values.range)throw Error("Fine and Range need different buttons");emit({kind:"roll_bindings",bindings:values});});
    dialogNote("These mappings apply in Piano Roll. Existing Synth Edit and Shift roles take priority if a button conflicts. Left stick navigates; right stick resizes or adjusts the focused value.");
  }
  function pitchDisplayDialog(){form("Pitch display & entry",[
    {name:"intervals",label:"Pitch labels",value:model.pitchIntervals,options:[{value:false,label:"Notes"},{value:true,label:"Intervals + octave"}]},
    {name:"chromatic",label:"Authored pitch stepping",value:model.chromaticEntry,options:[{value:false,label:"Follow phrase scale"},{value:true,label:"Chromatic · semitones"}]}
  ],values=>emit({kind:"pitch_display",intervals:values.intervals==="true",chromatic:values.chromatic==="true"}));dialogNote("Display changes preserve stored pitches. Piano Roll edits always step by semitones; exact pitch entry accepts accidentals.");}
  function parseNote(text){if(text.trim()===""||text==="—")return null;const match=/^([A-Ga-g])([#b♭]?)(-?\d+)$/.exec(text.trim());if(!match)throw Error("Use a note name such as C4, F#3 or Bb2");const pitch=({C:0,D:2,E:4,F:5,G:7,A:9,B:11})[match[1].toUpperCase()]+(match[2]==="#"?1:match[2]? -1:0)+(Number(match[3])+1)*12;if(pitch<0||pitch>127)throw Error("Note is outside MIDI range");return pitch;}
  function noteName(pitch){return ["C","C#","D","Eb","E","F","F#","G","Ab","A","Bb","B"][pitch%12]+(Math.floor(pitch/12)-1);}
  const fxTypes=["none","offset","velocity","gate","cut","delay","retrigger","chance","cc","bend","pressure","poly_pressure","program","tempo","jump","chord","add","voicing","spread","send"];
  function chordRevisionLabel(id){return (model.chordBank?.custom?.[id]?.name||"Chord")+" · revision "+id;}
  function chordSummary(chord){
    if(!chord)return "No chord selected";
    const definition=chord.definition,verified=chord.verification;
    return `${chord.name} · intervals ${definition.shape.join(", ")} · inversion ${definition.inversion} · ${definition.open?"open":"closed"} · spread ${definition.spread} · verified at ${noteName(verified.root)}: ${verified.pitches.map(noteName).join(" · ")}`;
  }
  function dialogNote(text){const note=document.createElement("p");note.className="wide dialog-note";note.setAttribute("role","status");note.textContent=text;$("tracker-fields").append(note);return note;}
  function chordBankDialog(){
    const context={revision:model.revision,occurrence:model.occurrence,phrase:model.phrase,column:model.column,at:phraseEditBeat(),selected:model.selectedEvents||[],bank:model.chordBank};
    const entries=[...(context.bank?.defaults||[]).map((chord,index)=>({value:"default:"+index,label:"Default · "+chord.name,chord,source:{kind:"default",index}})),...Object.entries(context.bank?.custom||{}).map(([id,chord])=>({value:"saved:"+id,label:"Custom · "+chord.name+" · revision "+id,chord,source:{kind:"saved",revision:unsignedInteger(id)}}))];
    const canApply=context.occurrence&&context.phrase?.value.events.some(event=>event.note),actions=[...(canApply&&entries.length?[{value:"apply",label:"Apply to an existing note"}]:[]),...(entries.length?[{value:"copy",label:"Copy / edit as a new Custom revision"}]:[]),{value:"new",label:"Create a Custom chord"},...(entries.length?[{value:"publish",label:"Publish selected chord to shared library"}]:[])];
    form("Chord bank · "+(context.bank?.scale||model.key),[{name:"query",label:"Search name or intervals",value:"",required:false},{name:"source",label:"Default / Custom",value:entries[0]?.value??"",options:entries.length?entries:[{value:"",label:"No saved chords"}]},{name:"action",label:"Action",value:actions[0].value,options:actions}],values=>{
      const entry=entries.find(entry=>entry.value===values.source);
      if(values.action!=="new"&&!entry)throw Error("Choose a chord");
      if(values.action==="publish"){emit({kind:"chord_library",action:{kind:"publish",revision:context.revision,source:entry.source}});return;}
      queueMicrotask(()=>values.action==="apply"?applyChordDialog(entry,context):chordEditorDialog(values.action==="copy"?entry.chord:null,context));
    });
    const source=[...$("tracker-fields").querySelectorAll("select")].find(input=>input.name==="source"),summary=dialogNote("");
    source.dataset.controllerChord="true";focusController(source);
    source.onchange=()=>{summary.textContent=chordSummary(entries.find(entry=>entry.value===source.value)?.chord);};source.onchange();
    const query=$("tracker-fields").querySelector('[name="query"]');
    query.oninput=()=>{const words=query.value.toLowerCase().trim().split(/\s+/).filter(Boolean),selected=source.value;source.replaceChildren();
      for(const entry of entries){const text=(entry.label+" "+entry.chord.definition.shape.join(",")).toLowerCase();if(!words.every(word=>text.includes(word)))continue;const option=document.createElement("option");option.value=entry.value;option.textContent=entry.label;source.append(option);}
      if([...source.options].some(option=>option.value===selected))source.value=selected;source.onchange();};
    dialogNote("Default chords follow this phrase’s effective scale. Custom chords are immutable: saving a copy always creates a new revision.");
  }
  function chordLibraryDialog(){
    const revision=model.revision;
    form("Custom chord library · all Songs",[{name:"digest",label:"Immutable revision",value:"",options:[{value:"",label:"Loading library…"}]}],values=>{
      if(!values.digest)throw Error("Choose a library revision");
      emit({kind:"chord_library",action:{kind:"adopt",revision,digest:values.digest}});
    });
    activeChordLibrary=true;chordLibraryKey=null;
    dialogNote("Adopt copies this exact revision into the current Song. Later library changes never alter saved music.");
    emit({kind:"chord_library",action:{kind:"refresh"}});updateChordLibrary();
  }
  function updateChordLibrary(){
    if(!activeChordLibrary||!$("tracker-dialog").open)return;
    const state=model.chordLibrary||{entries:[]},key=stringify(state);if(key===chordLibraryKey)return;chordLibraryKey=key;
    const select=$("tracker-fields").querySelector('[name="digest"]');if(!select)return;
    const previous=select.value;select.replaceChildren();
    for(const entry of state.entries){const option=document.createElement("option");option.value=entry.digest;option.textContent=entry.chord.name+" · "+entry.chord.definition.shape.join(",")+" · "+entry.digest.slice(0,10);select.append(option);}
    if(!state.entries.length){const option=document.createElement("option");option.value="";option.textContent=state.loading?"Loading…":"No published chords. Publish one from Chord Bank.";select.append(option);}
    if([...select.options].some(option=>option.value===previous))select.value=previous;
    $("tracker-form-error").textContent=state.error||"";$("tracker-form").querySelector('[type="submit"]').disabled=state.loading||!state.entries.length||!!state.error;
  }
  function readChordDraft(inputs){
    const get=name=>inputs.find(input=>input.name===name).value,intervalText=get("intervals").trim();
    if(!intervalText||intervalText.split(",").some(part=>!/^[-+]?\d+$/.test(part.trim())))throw Error("Enter comma-separated integer intervals, from -12 to 12");
    const intervals=intervalText.split(",").map(Number);
    if(intervals.some(interval=>interval < -12||interval > 12))throw Error("Every interval must be from -12 to 12");
    const root=parseNote(get("root")),inversion=Number(get("inversion")),spread=Number(get("spread"));
    if(root===null)throw Error("Declare a root note and octave, such as C4");
    if(!Number.isInteger(inversion)||!Number.isInteger(spread)||inversion<0||inversion>255||spread<0||spread>255)throw Error("Inversion and spread must be whole numbers from 0 to 255");
    return {name:get("name"),intervals,root,inversion,open:get("open")==="true",spread};
  }
  function chordEditorDialog(chord,context){
    const definition=chord?.definition;
    form(chord?"Copy chord as new revision":"New Custom chord",[{name:"name",label:"Name",value:chord?.name??"My chord"},{name:"intervals",label:"Literal semitone intervals (-12 to 12)",value:definition?.shape.join(",")??"0,4,7"},{name:"root",label:"Declared root + octave",value:noteName(chord?.verification.root??60)},{name:"inversion",label:"Inversion",value:definition?.inversion??0,type:"number",min:0,max:255},{name:"open",label:"Voicing",value:definition?.open??false,options:[{value:false,label:"Closed"},{value:true,label:"Open"}]},{name:"spread",label:"Additional spread voices",value:definition?.spread??0,type:"number",min:0,max:255}],()=>emit({kind:"save_chord",revision:context.revision,draft:readChordDraft(inputs)}));
    const inputs=[...$("tracker-fields").querySelectorAll("input,select")],output=dialogNote("Verifying pitches…");
    const preview=()=>{const token=++chordPreviewToken;chordDraftPreview={token,revision:context.revision,output};try{const draft=readChordDraft(inputs);output.textContent="Verifying pitches…";emit({kind:"preview_chord",revision:context.revision,token,draft});}catch(error){output.textContent=error.message;}};
    for(const input of inputs){input.oninput=preview;input.onchange=preview;}preview();
  }
  function updateChordPreview(preview){
    const current=chordDraftPreview;
    if(!current||!$("tracker-dialog").open||preview?.token!==current.token||preview?.revision!==current.revision)return;
    current.output.textContent=preview.error||chordSummary(preview.chord);
  }
  function applyChordDialog(entry,context){
    const notes=context.phrase.value.events.filter(event=>event.note);
    const first=notes.find(event=>context.selected.includes(event.id))||notes.find(event=>compareBeats(event.at,context.at)===0)||notes[0];
    const slotFor=event=>context.column>=3&&context.column<=5?context.column-3:Math.max(0,event.fx.findIndex(fx=>!fx));
    const slotOptions=event=>event.fx.map((fx,index)=>({value:index,label:`FX ${index+1} · ${fx?fxChoice(fx).startsWith("bank:")?chordRevisionLabel(fx.revision):fxChoice(fx).toUpperCase()+" "+fxValue(fx):"empty"}`}));
    form("Apply "+entry.chord.name,[{name:"event",label:"Existing note",value:first.id,options:notes.map(event=>({value:event.id,label:`${noteName(event.note.pitch)} · beat ${beatLabel(event.at)} · event ${event.id}`}))},{name:"slot",label:"Replace this FX slot only",value:slotFor(first),options:slotOptions(first)},{name:"root",label:"Playback root + octave",value:noteName(first.note.pitch)}],values=>{
      const root=parseNote(values.root);if(root===null)throw Error("Choose a root note and octave");
      emit({kind:"apply_chord",revision:context.revision,occurrence:context.occurrence,event:unsignedInteger(values.event),slot:Number(values.slot),root,source:entry.source});
    });
    const inputs=[...$("tracker-fields").querySelectorAll("input,select")],eventInput=inputs.find(input=>input.name==="event"),slot=inputs.find(input=>input.name==="slot"),root=inputs.find(input=>input.name==="root");
    eventInput.onchange=()=>{const event=notes.find(event=>String(event.id)===eventInput.value);slot.replaceChildren();for(const choice of slotOptions(event)){const option=document.createElement("option");option.value=String(choice.value);option.textContent=choice.label;slot.append(option);}slot.value=String(slotFor(event));root.value=noteName(event.note.pitch);};
    dialogNote(chordSummary(entry.chord));
    dialogNote("Other FX slots are preserved. Explicit VOICING / SPREAD FX override the chord’s saved defaults. Applying a Default chord saves its revision and edits this note in one undo step.");
  }
  function fxChoice(fx){return fx?.kind==="chord_revision"?"bank:"+fx.revision:fx?.kind==="control"?fx.control.kind:fx?.kind??"none";}
  function fxValue(fx){return fx?.kind==="chord_revision"?chordRevisionLabel(fx.revision):formatFx(fx);}
  function fxValueLabel(kind){
    if(kind.startsWith("bank:"))return "Saved chord revision · settings retained";
    return ({none:"No effect",offset:"Pitch offset · semitones (−128 to 127)",velocity:"Attack velocity · 1–127",gate:"Gate · fraction of note duration (e.g. 1/2)",cut:"Cut after · quarter-note beats",delay:"Delay · quarter-note beats",retrigger:"Count, spacing in beats (e.g. 3,1/6)",chance:"Probability · percent (0–100)",cc:"Controller number, value (0–127 each)",bend:"Pitch bend · raw 0–16383 (center 8192)",pressure:"Channel pressure · raw 0–127",poly_pressure:"MIDI pitch, pressure (0–127 each)",program:"Program · raw 0–127",tempo:"Tempo · BPM",jump:"Destination row · starts at 1",chord:"Literal chord · semitone intervals (e.g. 0,4,7)",add:"Added voices · intervals (−12 to 12)",voicing:"Inversion, open voicing (e.g. 1,true)",spread:"Octave spread · number of voices",send:"Send bus ID, amount %, beats (e.g. 0,65,1/2)"})[kind]||"Value";
  }
  function noteDialog(row=model.row,selected=null,pitchHint="C4",insert=false,startAt=null){if(!model.phrase)return;const phrase=model.phrase.value,at=selected?.at??startAt??phraseEditBeat(phrase,row),existing=insert?null:selected||phrase.events.find(e=>compareBeats(e.at,at)===0),note=existing?.note;
    const targets=Object.entries(model.instruments).map(([id,instrument])=>({value:"instrument:"+id,label:String(id).padStart(2,"0")+" / "+instrument.name}));for(let c=1;c<=16;c++)targets.push({value:"channel:"+c,label:"INTERNAL MIDI CHANNEL "+c});
    for(const endpoint of model.outputs?.endpoints||[])targets.push({value:"external:"+encodeURIComponent(endpoint.id),label:"EXTERNAL · "+endpoint.name});
    const currentTarget=existing?.target?.endpoint?"external:"+encodeURIComponent(existing.target.endpoint):existing?.target?.kind==="instrument"?"instrument:"+existing.target.id:existing?.target?"channel:"+existing.target.channel:targets[0]?.value;
    if(currentTarget&&!targets.some(target=>target.value===currentTarget))targets.push({value:currentTarget,label:"CURRENT OUTPUT (OFFLINE)"});
    const stepBeat=phrase.form.kind==="piano_roll"?model.pianoControls.grid:gridBeat(phrase),fields=[{name:"action",label:"Action",value:"update",options:[{value:"update",label:"Save note"},...(existing?[{value:"delete",label:"Delete this event"}]:[])]},{name:"start",label:"Start (quarter-note beats)",value:beatLabel(existing?.at??at)},{name:"pitch",label:"Note (empty for FX-only)",value:note?noteName(note.pitch):existing?"":pitchHint,required:false},{name:"target",label:"Instrument / MIDI channel",value:currentTarget,options:targets},{name:"channel",label:"Output channel (1–16)",value:existing?.target?.channel??1,type:"number",min:1,max:16},{name:"velocity",label:"Attack velocity (1–127)",value:note?.velocity??100,type:"number",min:1,max:127},{name:"release_velocity",label:"Release velocity (0–127)",value:note?.release_velocity??0,type:"number",min:0,max:127},{name:"duration",label:"Duration (quarter-note beats)",value:note?beatLabel(note.duration):beatLabel(scaleBeat(stepBeat,9,10))}];
    for(let i=0;i<3;i++){const fx=existing?.fx[i],kind=fxChoice(fx),choices=[...new Set([...fxTypes,...Object.keys(model.chordBank?.custom||{}).map(id=>"bank:"+id),kind])];fields.push({name:"fx"+i,label:"FX "+(i+1),value:kind,options:choices.map(value=>({value,label:value.startsWith("bank:")?"CRD · "+chordRevisionLabel(value.slice(5)):value.toUpperCase()}))},{name:"value"+i,label:"Value · bank chords use their saved settings",value:fxValue(fx),required:false});}
    form(existing?eventLabel(existing):phrase.form.kind==="piano_roll"?"New note · beat "+beatLabel(at):"Row "+(row+1),fields,v=>{if(v.action==="delete"){edit({kind:"delete_events",phrase:model.phrase.id,events:[existing.id]});return;}const pitch=parseNote(v.pitch),[kind,targetId]=v.target.split(":");const target=kind==="instrument"?{kind,id:unsignedInteger(targetId),channel:Number(v.channel)}:kind==="external"?{kind:"channel",channel:Number(v.channel),endpoint:decodeURIComponent(targetId)}:{kind,channel:Number(targetId),endpoint:null};
      const id=existing?.id??model.nextIdentity;
      const expression=(note?.expression||[]).map(point=>note.pitch!==pitch&&point.control.kind==="poly_pressure"?{...point,control:{...point.control,pitch}}:point);
      const event={id,at:parseBeat(v.start),target,note:pitch===null?null:{...(existing?.note??{}),pitch,velocity:Number(v.velocity),release_velocity:Number(v.release_velocity),duration:note?.duration??parseBeat(v.duration),expression},controls:existing?.controls??[],fx:[0,1,2].map(i=>{const fx=existing?.fx[i],kind=fxChoice(fx);return v["fx"+i]===kind&&v["value"+i]===fxValue(fx)?structuredClone(fx??null):parseFx(v["fx"+i],v["value"+i]);})};const put={kind:"put_event",phrase:model.phrase.id,event};edit(note&&pitch!==null?{kind:"batch",commands:[put,{kind:"resize_notes",phrase:model.phrase.id,events:[id],duration:parseBeat(v.duration)}]}:put);});
    const inputs=[...$("tracker-fields").querySelectorAll("input,select")];
    for(let i=0;i<3;i++){const kind=inputs.find(input=>input.name==="fx"+i),value=inputs.find(input=>input.name==="value"+i);let wasBank=false;const update=()=>{const isBank=kind.value.startsWith("bank:"),label=fxValueLabel(kind.value);value.disabled=isBank||kind.value==="none";value.parentElement.firstChild.textContent=label;value.setAttribute("aria-label",label);if(isBank)value.value=chordRevisionLabel(kind.value.slice(5));else if(wasBank)value.value="";wasBank=isBank;};kind.onchange=update;update();}
  }
  function previewEdit(command,title,revision){
    const token=++editPreviewToken;
    form(title+" · preview",[],()=>{
      if(!pendingEditPreview?.ready)throw Error("Wait for a valid preview");
      emit({kind:"edit",revision,command});
    });
    const output=dialogNote("Checking exact changes…");
    pendingEditPreview={token,revision,output,ready:false};
    $("tracker-form").querySelector('[type="submit"]').disabled=true;
    emit({kind:"preview_edit",revision,token,command});
  }
  function updateEditPreview(preview){
    const pending=pendingEditPreview;if(!pending||!$("tracker-dialog").open||preview?.token!==pending.token)return;
    pending.ready=!preview.error&&preview.revision===model.revision;
    const submit=$("tracker-form").querySelector('[type="submit"]');submit.disabled=!pending.ready;
    if(!pending.ready){pending.output.textContent=preview.error||"Song changed; cancel and reopen this preview.";return;}
    const changes=preview.changes;
    const describe=event=>event?`${event.pitch===null?"CONTROL":noteName(event.pitch)} @ ${beatLabel(event.at)}`+(event.duration?` · length ${beatLabel(event.duration)} · ${event.expressionPoints} expression points`:""):"REMOVED";
    pending.output.textContent=`${changes.changed} changed · ${changes.removed} removed · ${changes.added} added. Phrase length ${beatLabel(changes.beforeLength)} to ${beatLabel(changes.afterLength)} beats. `;
    const list=document.createElement("ul");for(const change of changes.events){const item=document.createElement("li");item.textContent=`#${change.event}: ${describe(change.before)} → ${describe(change.after)}`;list.append(item);}pending.output.append(list);
    if(changes.changed>changes.events.length)pending.output.append(document.createTextNode("First 32 changes shown. Apply commits the complete transaction; Undo restores it."));
  }
  function phraseDialog(){if(!model.phrase)return;const p=model.phrase.value,division=divisionOf(p);
    form("Phrase settings",[{name:"name",label:"Name",value:p.name},{name:"steps",label:"Length (steps)",value:p.steps,type:"number",min:1},{name:"division",label:"Division",value:division.denominator,options:[4,8,16,32,64].map(v=>({value:v,label:"1/"+v}))},{name:"tuplet",label:"Tuplet notes / in time of",value:division.notes+"/"+division.in_time_of},{name:"trim",label:"When shortening",value:"false",options:[{value:"false",label:"Reject if events would be trimmed"},{value:"true",label:"Trim events and durations past end"}]},{name:"type",label:"Phrase form",value:p.form.kind,options:["tracker","piano_roll","drum","euclidean","random"].map(value=>({value,label:value.toUpperCase()}))}],v=>{const phrase=structuredClone(p),tuplet=v.tuplet.split("/").map(Number);phrase.name=v.name;phrase.steps=Number(v.steps);phrase.division={denominator:Number(v.division),notes:tuplet[0],in_time_of:tuplet[1]};
      const first=unsignedInteger(Object.keys(model.instruments)[0]??"0");if(!first)throw Error("Choose an instrument first");const target={kind:"instrument",id:first,channel:1};
      if(v.type!==p.form.kind)phrase.form=v.type==="euclidean"?{kind:v.type,pulses:Math.min(4,phrase.steps),rotation:0,pitch:60,velocity:100,gate:beat(9,10),target}:v.type==="random"?{kind:v.type,seed:1,density:60,low:48,high:72,velocity_low:70,velocity_high:110,gate:beat(9,10),target}:{kind:v.type};
      const commands=[];
      const divisionChanged=["denominator","notes","in_time_of"].some(key=>phrase.division[key]!==division[key]);
      const desiredSteps=phrase.steps,desiredDivision=phrase.division;
      phrase.steps=p.steps;phrase.division=p.division;
      commands.push({kind:"put_phrase",id:model.phrase.id,phrase});
      if(divisionChanged)commands.push({kind:"retime_division",phrase:model.phrase.id,division:desiredDivision});
      if(desiredSteps!==p.steps)commands.push({kind:"resize_phrase",phrase:model.phrase.id,steps:desiredSteps,trim:v.trim==="true"});
      const command={kind:"batch",commands},revision=formRevision;
      if(divisionChanged||desiredSteps<p.steps)queueMicrotask(()=>previewEdit(command,"Phrase timing changes",revision));else edit(command);});
  }
  function scaleDialog(phrase=false){
    // Event listeners pass their event as the first argument.
    phrase=phrase===true;
    const local=phrase?model.phrase?.value.key:null,current=local||model.scale;
    if(phrase&&!model.phrase)return;
    const options=[...(phrase?[{value:"inherit",label:"Inherit Song key"}]:[]),...model.scaleCatalog,{value:"custom",label:"Custom pitch classes"}];
    form(phrase?"Phrase key / scale":"Song key / scale",[
      {name:"root",label:"Root",value:current.root,options:["C","C#","D","Eb","E","F","F#","G","Ab","A","Bb","B"].map((label,value)=>({label,value}))},
      {name:"scale",label:"Scale",value:phrase&&!local?"inherit":current.kind,options},
      {name:"classes",label:"Custom absolute pitch classes · C=0 through B=11",value:current.pitch_classes.join(",")},
      {name:"spelling",label:"Accidental spelling",value:current.spelling,options:[{value:"flats",label:"Flats"},{value:"sharps",label:"Sharps"}]}
    ],values=>{
      const pitch_classes=values.classes.split(",").map(text=>Number(text.trim()));
      if(values.scale==="custom"&&pitch_classes.some(value=>!Number.isInteger(value)||value<0||value>11))throw Error("Pitch classes must be integers from 0 to 11");
      emit({kind:"key_settings",revision:formRevision,phrase,root:Number(values.root),scale:values.scale==="inherit"?null:values.scale,pitch_classes:values.scale==="custom"?pitch_classes:[],spelling:values.spelling});
    });
  }
  function makeUnique(){edit({kind:"make_unique",occurrence:model.occurrence});}
  const capture=action=>emit({kind:"capture",action});
  function resourceRepairDialog(){
    const candidate=model.resourceRepair;if(!candidate)return;
    form("Repair assets · "+candidate.name,[
      {name:"index",label:candidate.error,value:candidate.resources?.[0]?.index,options:(candidate.resources||[]).map(item=>({value:item.index,label:item.name+(item.sha256?" · "+item.sha256.slice(0,12):" · no saved digest")}))},
      {name:"action",label:"Locate requires identical content. Replace creates new revisions for all affected pins; Save As preserves original Song.",value:"locate",options:[{value:"locate",label:"Locate original content"},{value:"replace",label:"Replace all references with new content"},{value:"cancel",label:"Cancel Open; retain current Song"}]}
    ],v=>emit({kind:"repair_resource",index:Number(v.index)||0,action:v.action}));
  }
  function outputDialog(){
    emit({kind:"midi_output",action:"discover",endpoint:null});
    form("External MIDI output",[{name:"endpoint",label:"Exact destination",value:model.outputs?.endpoints?.[0]?.id,options:(model.outputs?.endpoints||[]).map(v=>({value:v.id,label:v.name}))},{name:"action",label:"Action",value:"connect",options:[{value:"connect",label:"Connect"},{value:"disconnect",label:"Disconnect"},{value:"mpe",label:"Configure MPE destination zones"},{value:"discover",label:"Refresh ports"}]}],v=>{
      if(v.action==="mpe"){if(!v.endpoint)throw Error("Choose an exact output first");queueMicrotask(()=>mpeOutputDialog(v.endpoint));}
      else emit({kind:"midi_output",action:v.action,endpoint:v.endpoint||null});
    });
  }
  function mpeOutputDialog(endpoint){
    const profile=(model.preferences?.mpe_outputs||[]).find(value=>value.endpoint===endpoint),zones=profile?.zones||[];
    form("Declare MPE output zones",[
      {name:"lower",label:"Lower-zone members · master 1 · 0 disables",value:zones.find(zone=>zone.kind==="lower")?.members??(profile?0:15),type:"number",min:0,max:15},
      {name:"upper",label:"Upper-zone members · master 16 · 0 disables",value:zones.find(zone=>zone.kind==="upper")?.members??0,type:"number",min:0,max:15}
    ],values=>{
      const lower=Number(values.lower),upper=Number(values.upper);
      if(![lower,upper].every(value=>Number.isInteger(value)&&value>=0&&value<=15)||lower&&upper&&lower+upper>14)throw Error("Zones must not overlap; two zones allow at most 14 member channels total");
      const zones=[...(lower?[{kind:"lower",members:lower}]:[]),...(upper?[{kind:"upper",members:upper}]:[])];
      emit({kind:"mpe_output_profile",endpoint,zones});
    });
    dialogNote("Declare only a receiver that supports MPE 1.0 master/member bend, pressure, CC74, RPN0 ranges and sustain, and retains released-voice expression across channel reuse. One independent occurrence or monitor session leases one zone. Exhaustion rejects visibly; no voice stealing. Set both counts to 0 to remove this profile. Physical receiver compatibility requires validation.");
  }
  function preferencesDialog(){
    const settings=model.preferences||{},clock=settings.clock||{},active=settings.active_input,mpe=settings.mpe_input;
    const outputs=(model.outputs?.endpoints||[]).filter(endpoint=>(model.outputs?.connected||[]).includes(endpoint.id));
    const inputOptions=[{value:"internal",label:"Internal tempo"},...(active?[{value:"input",label:"Selected MIDI input · "+active.name}]:[])];
    if(clock.input&&!active)inputOptions.push({value:"offline",label:"Saved MIDI input is offline · choose Internal"});
    const outputOptions=[{value:"",label:"No MIDI clock output"},...outputs.map(endpoint=>({value:endpoint.id,label:endpoint.name+" · "+endpoint.id}))];
    if(clock.output&&!outputs.some(endpoint=>endpoint.id===clock.output))outputOptions.push({value:clock.output,label:"Saved MIDI output is offline · choose another"});
    const mpeSources=[{value:"",label:"Ordinary MIDI only"},...(active?[{value:active.id,label:"MPE · "+active.name}]:[])];
    if(mpe&&!mpeSources.some(item=>item.value===mpe.source))mpeSources.push({value:mpe.source,label:"Saved MPE source · offline or different input"});
    form("Machine defaults & MIDI clock",[{name:"default_bpm",label:"Default tempo for new songs (BPM)",value:settings.default_bpm??120,type:"number",min:0.001,max:4294967.295,step:0.001},{name:"monitor",label:"Default recording monitor",value:settings.monitor??false,options:[{value:false,label:"Off"},{value:true,label:"On"}]},{name:"source",label:"Playback clock source",value:clock.input?active?"input":"offline":"internal",options:inputOptions},{name:"output",label:"Clock destination · connected exact port",value:clock.output??"",options:outputOptions},{name:"delay_ms",label:"External input clock latency (milliseconds)",value:clock.delay_ms??100,type:"number",min:20,max:2000},
      {name:"mpe_source",label:"Declared MPE input source",value:mpe?.source??"",options:mpeSources},
      {name:"mpe_zone",label:"MPE zone",value:mpe?.zone.kind??"lower",options:[{value:"lower",label:"Lower · master channel 1"},{value:"upper",label:"Upper · master channel 16"}]},
      {name:"mpe_members",label:"MPE member channels",value:mpe?.zone.members??15,type:"number",min:1,max:15},
      {name:"mpe_member_range",label:"Member bend range · semitones,cents",value:(mpe?.member_range.semitones??48)+","+(mpe?.member_range.cents??0)},
      {name:"mpe_master_range",label:"Master bend range · semitones,cents",value:(mpe?.master_range.semitones??2)+","+(mpe?.master_range.cents??0)}],values=>{
      const default_tempo=parseTempoMilliBpm(values.default_bpm),delay_ms=Number(values.delay_ms),input_clock=values.source==="input",output_clock=values.output||null;
      if(!Number.isInteger(delay_ms)||delay_ms<20||delay_ms>2000)throw Error("Input clock latency must be a whole number from 20 to 2000 milliseconds");
      if(values.source==="offline"||input_clock&&!active)throw Error("Select a connected MIDI input in Synth, or choose Internal tempo");
      if(input_clock&&output_clock)throw Error("Choose MIDI clock input or output; simultaneous input and output would echo clock");
      if(output_clock&&!outputs.some(endpoint=>endpoint.id===output_clock))throw Error("Choose a connected MIDI output, or disable clock output");
      const range=text=>{const parts=text.split(",").map(Number);if(parts.length!==2||!parts.every(Number.isInteger)||parts[0]<0||parts[0]>127||parts[1]<0||parts[1]>99)throw Error("Bend range requires 0–127 semitones and 0–99 cents");return{semitones:parts[0],cents:parts[1]};};
      const mpe_input=values.mpe_source?{source:values.mpe_source,zone:{kind:values.mpe_zone,members:Number(values.mpe_members)},member_range:range(values.mpe_member_range),master_range:range(values.mpe_master_range)}:null;
      emit({kind:"preferences",default_tempo,monitor:values.monitor==="true",input_clock,output_clock,delay_ms,mpe_input});
    });
    dialogNote("External clock schedules completed pulse intervals with this fixed latency. Default tempo applies to new songs. MIDI input selection stays in Synth’s device settings.");
    if(settings.status)dialogNote(settings.status);
  }
  function soundLibraryDialog(){form("Sound library",[{name:"instrument",label:"Instrument",value:Object.keys(model.instruments)[0],options:Object.entries(model.instruments).map(([value,instrument])=>({value,label:String(value).padStart(2,"0")+" / "+instrument.name}))},{name:"action",label:"Action",value:"publish",options:[{value:"publish",label:"Publish current sound"},{value:"adopt",label:"Adopt a published revision"}]}],v=>emit({kind:"sound_library",instrument:unsignedInteger(v.instrument),action:v.action}));}
  const midiControlActions=["up","down","left","right","confirm","back","play","stop","undo","redo","menu","song","instruments","record","end_take","increase","decrease"];
  function midiControlsDialog(){
    const bindings=structuredClone(model.preferences?.midi_controls||[]);
    form("MIDI control surface",[
      {name:"binding",label:"Mapping",value:0,options:bindings.length?bindings.map((binding,index)=>({value:index,label:`${binding.trigger.toUpperCase()} ${binding.number} · ch ${binding.channel} · ${binding.action.replaceAll("_"," ")} · ${binding.source}`})):[{value:"",label:"No mappings; all performance messages pass through"}]},
      {name:"action",label:"Action",value:"add",options:[{value:"add",label:"Add mapping"},...(bindings.length?[{value:"edit",label:"Edit selected mapping"},{value:"delete",label:"Delete selected mapping"},{value:"reset",label:"Reset to defaults (no mappings)"}]:[])]}
    ],values=>{
      const index=Number(values.binding);
      if(values.action==="delete"){bindings.splice(index,1);emit({kind:"midi_controls",bindings});return;}
      if(values.action==="reset"){emit({kind:"midi_controls",bindings:[]});return;}
      queueMicrotask(()=>midiBindingDialog(bindings,values.action==="add"?null:index));
    });
    dialogNote("Mappings act as buttons: positive Note On presses, or CC crossing from below 64 to 64 or above. Releases re-arm the button. Mapped packets are consumed and excluded from performance recording. Choose the physical MIDI input in Synth’s device settings.");
    if(model.preferences?.status)dialogNote(model.preferences.status);
  }
  function midiBindingDialog(bindings,index){
    const active=model.preferences?.active_input,binding=index===null?{source:active?.id??"",channel:1,trigger:"cc",number:20,action:"play"}:bindings[index];
    const sources=new Map(bindings.map(item=>[item.source,item.source]));if(active)sources.set(active.id,active.name+" · "+active.id);
    form(index===null?"Add MIDI action mapping":"Edit MIDI action mapping",[
      {name:"source",label:"Exact MIDI source",value:binding.source,options:sources.size?[...sources].map(([value,label])=>({value,label})):[{value:"",label:"Select a MIDI input in Synth first"}]},
      {name:"channel",label:"MIDI channel (1–16)",value:binding.channel,type:"number",min:1,max:16},
      {name:"trigger",label:"Button message",value:binding.trigger,options:[{value:"note",label:"Note On (velocity > 0)"},{value:"cc",label:"CC (rising value ≥ 64)"}]},
      {name:"number",label:"Note / CC number (0–127)",value:binding.number,type:"number",min:0,max:127},
      {name:"action",label:"Tracker action",value:binding.action,options:midiControlActions.map(value=>({value,label:value.replaceAll("_"," ").toUpperCase()}))}
    ],values=>{
      const candidate={source:values.source,channel:Number(values.channel),trigger:values.trigger,number:Number(values.number),action:values.action};
      if(!candidate.source)throw Error("Select a physical MIDI input in Synth first");
      if(!Number.isInteger(candidate.channel)||candidate.channel<1||candidate.channel>16||!Number.isInteger(candidate.number)||candidate.number<0||candidate.number>127)throw Error("Choose channel 1–16 and note / CC number 0–127");
      if(bindings.some((other,otherIndex)=>otherIndex!==index&&other.source===candidate.source&&other.channel===candidate.channel&&other.trigger===candidate.trigger&&other.number===candidate.number))throw Error("Mapping conflict: this source, channel and button already has an action");
      const next=bindings.slice();if(index===null)next.push(candidate);else next[index]=candidate;
      emit({kind:"midi_controls",bindings:next});
    });
  }
  function bakeGenerator(form){emit({kind:"bake_generator",revision:model.revision,occurrence:model.occurrence,form});}
  function selectEvent(id,toggle=false){emit({kind:"select_events",revision:model.revision,occurrence:model.occurrence,events:[id],toggle});}
  function selectAllEvents(){emit({kind:"select_events",revision:model.revision,occurrence:model.occurrence,events:model.phrase.value.events.map(event=>event.id),toggle:false});}
  function clearEventSelection(){emit({kind:"select_events",revision:model.revision,occurrence:model.occurrence,events:[],toggle:false});}
  function cycleNote(direction){
    const notes=model.phrase.value.events.filter(event=>event.note).sort((a,b)=>compareBeats(a.at,b.at)||a.note.pitch-b.note.pitch||(a.id<b.id?-1:a.id>b.id?1:0));
    if(!notes.length){eventChooser();return;}
    const current=notes.findIndex(event=>(model.selectedEvents||[]).includes(event.id)),index=current<0?(direction<0?notes.length-1:0):(current+direction+notes.length)%notes.length;
    selectEvent(notes[index].id);
  }
  function eventChooser(action="select",requestedStart=null,focus="event"){
    const phrase=model.phrase.value,events=phrase.events,source={revision:model.revision,occurrence:model.occurrence};
    const current=events.find(event=>(model.selectedEvents||[]).includes(event.id))||events.find(event=>compareBeats(event.at,phraseEditBeat(phrase))===0)||events[0];
    const page=pageItems(events,requestedStart??Math.max(0,events.indexOf(current)));
    const choice=page.values.includes(current)?current:page.values[0],pages=Math.max(1,Math.ceil(events.length/EVENT_PAGE_SIZE));
    form("Choose note / event · overlaps keep distinct IDs",[
      {name:"page",label:`Event page (1–${pages} · ${events.length} events)`,value:Math.floor(page.start/EVENT_PAGE_SIZE)+1,type:"number",min:1,max:pages},
      {name:"event",label:"Exact event",value:choice?.id??"",options:events.length?page.values.map(event=>({value:event.id,label:eventLabel(event)+" · "+(event.note?"duration "+beatLabel(event.note.duration)+" · ":"")+targetChoice(event.target)+" · ch "+event.target.channel})):[{value:"",label:"No events yet"}]},
      {name:"action",label:"Action",value:events.length?action:"insert",options:[...(events.length?[{value:"select",label:"Select this event only"},{value:"toggle",label:"Add / remove from selection"},{value:"edit",label:"Edit this exact note / event"},{value:"raw",label:"Edit its raw MIDI / owned expression"},{value:"delete",label:"Delete this event"}]:[]),{value:"insert",label:"Insert a new note at cursor"}]}
    ],values=>{
      if(model.revision!==source.revision||!sameOccurrence(model.occurrence,source.occurrence))throw Error("Phrase changed while choosing an event; reopen the chooser");
      if(values.action==="insert"){queueMicrotask(()=>noteDialog(model.row,null,noteName(model.pianoControls.pitch),true,model.pianoControls.at));return;}
      const event=events.find(event=>String(event.id)===values.event);if(!event)throw Error("Choose an existing event");
      if(values.action==="select"||values.action==="toggle")emit({kind:"select_events",...source,events:[event.id],toggle:values.action==="toggle"});
      else if(values.action==="delete")emit({kind:"edit",revision:source.revision,command:{kind:"delete_events",phrase:model.phrase.id,events:[event.id]}});
      else queueMicrotask(()=>values.action==="raw"?performanceDialog(event):noteDialog(value(event.at)/grid(phrase),event));
    });
    fileInput("page").onchange=()=>{
      const number=Number(fileInput("page").value);
      if(!Number.isInteger(number)||number<1||number>pages){$("tracker-form-error").textContent=`Choose page 1–${pages}`;return;}
      if(model.revision!==source.revision||!sameOccurrence(model.occurrence,source.occurrence)){$("tracker-form-error").textContent="Phrase changed; reopen the chooser";return;}
      eventChooser(fileInput("action").value,(number-1)*EVENT_PAGE_SIZE,"page");
    };
    focusController(fileInput(focus));
    dialogNote(page.total?`Events ${page.start+1}–${page.end} of ${page.total}. Change Event page to reach every event without changing the phrase.`:"No events yet.");
    dialogNote("Left/Right selects a distinct event. Use Toggle to build a selection across overlapping notes; raw MIDI opens channel, member or master expression points.");
  }
  function selectionRequest(action,source={revision:model.revision,occurrence:model.occurrence}){emit({kind:"selection_edit",...source,action});}
  function selectionDialog(kind){
    const source={revision:model.revision,occurrence:model.occurrence},phrase=model.phrase.value,first=phrase.events.find(event=>(model.selectedEvents||[]).includes(event.id)&&event.note);
    const fields=kind==="move"?[{name:"amount",label:"Move by signed quarter-note beats",value:"+"+beatLabel(model.pianoControls.grid)}]:kind==="transpose"?[{name:"semitones",label:"Chromatic semitones (negative moves down)",value:1,type:"number",min:-127,max:127}]:kind==="duration"?[{name:"duration",label:"Set selected note durations (beats)",value:beatLabel(first?.note.duration||gridBeat(phrase))}]:kind==="quantize"?[{name:"grid",label:"Quantize grid (quarter-note beats)",value:beatLabel(model.pianoControls.grid)},{name:"durations",label:"What to quantize",value:"false",options:[{value:"false",label:"Onsets only; retain raw durations"},{value:"true",label:"Onsets and note durations"}]}]:[{name:"at",label:kind==="duplicate"?"Duplicate earliest selected event at beat":"Paste earliest copied event at beat",value:beatLabel(phraseEditBeat(phrase))}];
    form(kind.toUpperCase()+" · "+(model.selectedEvents?.length||0)+" selected events",fields,values=>{
      let action;
      if(kind==="move"){const text=values.amount.trim(),backward=text.startsWith("-");action={kind,amount:parseBeat(text.replace(/^[+-]/,"")),backward};}
      else if(kind==="transpose")action={kind,semitones:Number(values.semitones)};
      else if(kind==="duration")action={kind,duration:parseBeat(values.duration)};
      else if(kind==="quantize")action={kind,grid:parseBeat(values.grid),durations:values.durations==="true"};
      else action={kind:kind==="duplicate"?"duplicate":"paste",at:parseBeat(values.at)};
      if(kind==="quantize"){
        const command={kind:"quantize",phrase:model.phrase.id,events:[...(model.selectedEvents||[])],grid:action.grid,durations:action.durations};
        queueMicrotask(()=>previewEdit(command,"Quantize selection",source.revision));
      }else selectionRequest(action,source);
    });
  }
  function rangeSelectionDialog(){
    const phrase=model.phrase.value,source={revision:model.revision,occurrence:model.occurrence};
    form("Select time / pitch range",[
      {name:"start",label:"Start beat · inclusive",value:beatLabel(phraseEditBeat(phrase))},
      {name:"end",label:"End beat · exclusive",value:beatLabel(gridBeat(phrase,phrase.steps))},
      {name:"low",label:"Lowest pitch",value:"C-1"},{name:"high",label:"Highest pitch",value:"G9"},
      {name:"target",label:"Target filter",value:"all",options:[{value:"all",label:"All targets / channels"},...soundTargets()]},
      {name:"channel",label:"Channel for selected target",value:1,type:"number",min:1,max:16},
      {name:"controls",label:"Standalone controls in range",value:"false",options:[{value:"false",label:"Notes only"},{value:"true",label:"Include channel / master controls"}]}
    ],values=>emit({kind:"select_range",...source,range:{start:parseBeat(values.start),end:parseBeat(values.end),low:parseNote(values.low),high:parseNote(values.high),target:values.target==="all"?null:chosenTarget(values.target,values.channel),include_controls:values.controls==="true"}}));
  }
  function selectionMenu(){form("Selection actions",[{name:"action",label:"Action",value:"copy",options:[{value:"copy",label:"Copy selected events"},{value:"cut",label:"Cut selected events"},{value:"paste",label:"Paste at a beat"},{value:"duplicate",label:"Duplicate at a beat"},{value:"all",label:"Select all events"},{value:"range",label:"Select time / pitch range"},{value:"duration",label:"Set note durations"},{value:"delete",label:"Delete selected events"},{value:"clear",label:"Clear selection"}]}],values=>{if(values.action==="range")queueMicrotask(rangeSelectionDialog);else if(["paste","duplicate","duration"].includes(values.action))queueMicrotask(()=>selectionDialog(values.action));else if(values.action==="all")selectAllEvents();else if(values.action==="clear")clearEventSelection();else selectionRequest({kind:values.action});});}
  function controlLabel(control){return control.kind.toUpperCase()+(control.kind==="cc"?" "+control.number:control.kind==="poly_pressure"?" "+noteName(control.pitch):"")+" = "+control.value;}
  function eventLabel(event){return "#"+event.id+" · "+(event.note?noteName(event.note.pitch):"CONTROL / FX")+" @ "+beatLabel(event.at);}
  function performanceDialog(selected=null,requestedStart=null,scope="channel",focus="event"){
    if(!model.phrase)return;
    const phrase=model.phrase,source={revision:model.revision,phrase:phrase.id},events=phrase.value.events;
    const current=selected||events.find(event=>event.id===model.selectedEvents?.[0]);
    const page=pageItems(events,requestedStart??Math.max(0,events.indexOf(current)));
    const choice=page.values.includes(current)?current:requestedStart===null?null:page.values[0];
    form("Raw MIDI · choose source event",[
      ...rawPageFields(page,"Event"),
      {name:"event",label:"Event (distinct IDs preserve overlaps)",value:choice?.id??"new",options:[{value:"new",label:"New channel-control event"},...page.values.map(event=>({value:event.id,label:eventLabel(event)}))]},
      {name:"scope",label:"Ownership",value:scope,options:[{value:"channel",label:"Channel control at event onset"},{value:"expression",label:"Member expression at offset from attack"},{value:"master",label:"Owned MPE master expression"}]}
    ],values=>{
      requireCurrentRawSource(source);
      const existing=events.find(event=>String(event.id)===values.event)||null;
      if(!existing&&values.event!=="new")throw Error("Choose an existing event or a new channel control");
      if(values.scope!=="channel"&&!existing?.note)throw Error("Choose a note event for owned expression");
      if(values.scope==="master"&&!existing?.note?.mpe)throw Error("Choose a captured MPE note for its master expression");
      queueMicrotask(()=>performancePointDialog(existing,values.scope,source,phrase.value));
    });
    bindRawPage(page,source,start=>performanceDialog(current,start,fileInput("scope").value,"page"));
    focusController(fileInput(focus));
  }
  function rawPageFields(page,label){
    const pages=Math.max(1,Math.ceil(page.total/EVENT_PAGE_SIZE));
    return pages===1?[]:[{name:"page",label:`${label} page (1–${pages} · ${page.total} total)`,value:Math.floor(page.start/EVENT_PAGE_SIZE)+1,type:"number",min:1,max:pages}];
  }
  function requireCurrentRawSource(source){
    if(model.revision!==source.revision||model.phrase?.id!==source.phrase)throw Error("Phrase changed; reopen the raw MIDI editor");
  }
  function bindRawPage(page,source,open){
    const input=fileInput("page");if(!input)return;
    input.onchange=()=>{
      try{
        const number=Number(input.value),pages=Math.ceil(page.total/EVENT_PAGE_SIZE);
        if(!Number.isInteger(number)||number<1||number>pages)throw Error(`Choose page 1–${pages}`);
        requireCurrentRawSource(source);open((number-1)*EVENT_PAGE_SIZE);
      }catch(error){$("tracker-form-error").textContent=error.message;}
    };
    dialogNote(`Showing ${page.start+1}–${page.end} of ${page.total}. Change page to reach every item. Page navigation does not save edits.`);
  }
  function performancePointDialog(existing,scope,source,phrase,requestedStart=0,focus="entry"){
    const owned=scope!=="channel",points=owned?(scope==="master"?existing.note.master_expression||[]:existing.note.expression):existing?.controls||[];
    const page=pageItems(points,requestedStart),first=page.values[0],firstControl=owned?first?.control:first;
    const fields=[
      ...rawPageFields(page,owned?"Expression":"Control"),
      {name:"entry",label:owned?"Expression point":"Channel control",value:points.length?page.start:"new",options:[{value:"new",label:"Add new point"},...page.values.map((point,index)=>({value:page.start+index,label:`#${page.start+index+1} · `+(owned?beatLabel(point.offset)+" beats · ":"")+controlLabel(owned?point.control:point)}))]},
      {name:"action",label:"Action",value:"save",options:[{value:"save",label:"Save point"},{value:"delete",label:"Delete this point"}]},
      {name:"kind",label:"MIDI message",value:firstControl?.kind||"bend",options:["bend","pressure","poly_pressure","cc","program"].map(value=>({value,label:value.toUpperCase()}))},
      {name:"value",label:"Raw integer value (bend 0–16383; other 0–127)",value:firstControl?.value??8192,type:"number",min:0,max:16383},
      {name:"number",label:"Controller number (CC only)",value:firstControl?.number??1,type:"number",min:0,max:127},
      {name:"pitch",label:"MIDI pitch (poly pressure only)",value:firstControl?.pitch??existing?.note?.pitch??60,type:"number",min:0,max:127}
    ];
    if(owned)fields.push({name:"offset",label:"Offset from note attack (exact beats)",value:beatLabel(first?.offset||beat(0))});
    if(!existing){const target=defaultDrumTarget();fields.push({name:"at",label:"Start (exact quarter-note beats)",value:beatLabel(phraseEditBeat(phrase))},{name:"target",label:"Instrument / output",value:targetChoice(target),options:soundTargets()},{name:"channel",label:"Output channel",value:target.channel,type:"number",min:1,max:16});}
    const integer=(text,max,label)=>{const number=Number(text);if(!Number.isSafeInteger(number)||number<0||number>max)throw Error(label+" must be an integer from 0 to "+max);return number;};
    form((owned?scope==="master"?"Owned master expression":"Owned member expression":"Raw channel controls")+(existing?" · "+eventLabel(existing):""),fields,values=>{
      requireCurrentRawSource(source);
      const event=existing?structuredClone(existing):{id:model.nextIdentity,at:parseBeat(values.at),target:chosenTarget(values.target,values.channel),note:null,controls:[],fx:[null,null,null]};
      const entries=owned?(scope==="master"?(event.note.master_expression??=[]):event.note.expression):event.controls,index=values.entry==="new"?-1:Number(values.entry);
      if(values.action==="delete"){
        if(index<0)throw Error("Choose an existing point to delete");
        entries.splice(index,1);
      }else{
        const control={kind:values.kind,value:integer(values.value,values.kind==="bend"?16383:127,"MIDI value")};
        if(values.kind==="cc")control.number=integer(values.number,127,"CC number");
        if(values.kind==="poly_pressure")control.pitch=integer(values.pitch,127,"MIDI pitch");
        const sequence=owned?(index>=0?entries[index]?.sequence:nextExpressionSequence(event.note)):null;
        const point=owned?{offset:parseBeat(values.offset),control,...(sequence==null?{}:{sequence})}:control;
        if(index<0)entries.push(point);else entries[index]=point;
        if(owned)entries.sort((a,b)=>compareBeats(a.offset,b.offset));
      }
      const empty=!event.note&&!event.controls.length&&event.fx.every(fx=>!fx),command=empty?{kind:"delete_events",phrase:source.phrase,events:[event.id]}:{kind:"put_event",phrase:source.phrase,event};
      emit({kind:"edit",revision:source.revision,command});
    });
    const inputs=$("tracker-fields").querySelectorAll("input,select"),input=name=>[...inputs].find(node=>node.name===name);
    input("entry").onchange=()=>{
      const point=points[Number(input("entry").value)],control=owned?point?.control:point;
      input("kind").value=control?.kind||"bend";input("value").value=String(control?.value??8192);input("number").value=String(control?.number??1);input("pitch").value=String(control?.pitch??existing?.note?.pitch??60);
      if(owned)input("offset").value=beatLabel(point?.offset||beat(0));
    };
    input("kind").onchange=()=>{input("value").max=input("kind").value==="bend"?16383:127;if(Number(input("value").value)>Number(input("value").max))input("value").value=input("kind").value==="bend"?"8192":"0";};
    bindRawPage(page,source,start=>performancePointDialog(existing,scope,source,phrase,start,"page"));
    focusController(input(focus));
  }
  function soundTargets(){const options=Object.entries(model.instruments).map(([id,instrument])=>({value:"instrument:"+id,label:String(id).padStart(2,"0")+" / "+instrument.name}));for(let channel=1;channel<=16;channel++)options.push({value:"channel:"+channel,label:"INTERNAL MIDI CHANNEL "+channel});for(const endpoint of model.outputs?.endpoints||[])options.push({value:"external:"+encodeURIComponent(endpoint.id),label:"EXTERNAL · "+endpoint.name});return options;}
  function targetChoice(target){return target.endpoint?"external:"+encodeURIComponent(target.endpoint):target.kind==="instrument"?"instrument:"+target.id:"channel:"+target.channel;}
  function chosenTarget(choice,channel){const [kind,id]=choice.split(":");return kind==="instrument"?{kind,id:unsignedInteger(id),channel:Number(channel)}:kind==="external"?{kind:"channel",channel:Number(channel),endpoint:decodeURIComponent(id)}:{kind:"channel",channel:Number(id),endpoint:null};}
  const drumKey=lane=>lane.pitch+":"+targetChoice(lane.target)+":"+lane.target.channel;
  function defaultDrumTarget(){const id=unsignedInteger(Object.keys(model.instruments)[0]??"0");return id?{kind:"instrument",id,channel:1}:{kind:"channel",channel:1,endpoint:null};}
  function drumLanes(){
    const lanes=(model.drumLanes||[]).map((lane,index)=>({...lane,index,events:[]})),byKey=new Map(lanes.map(lane=>[drumKey(lane),lane]));
    for(const event of model.phrase.value.events){if(event.note)byKey.get(drumKey({pitch:event.note.pitch,target:event.target}))?.events.push(event);}
    return lanes;
  }
  function drumLaneDialog(lane=null){
    const source=model.occurrence,settings=lane||{pitch:36,target:defaultDrumTarget(),velocity:100,duration:beat(1,8)};
    form(lane?"Drum lane settings · applies to all lane notes":"Add drum lane · saved with its notes",[
      {name:"pitch",label:"Pitch",value:noteName(settings.pitch)},
      {name:"target",label:"Instrument / output",value:targetChoice(settings.target),options:soundTargets()},
      {name:"channel",label:"Output channel",value:settings.target.channel,type:"number",min:1,max:16},
      {name:"velocity",label:"Attack velocity",value:settings.velocity,type:"number",min:1,max:127},
      {name:"duration",label:"Duration (quarter-note beats)",value:beatLabel(settings.duration)}
    ],values=>{
      const draft={pitch:parseNote(values.pitch),target:chosenTarget(values.target,values.channel),velocity:Number(values.velocity),duration:parseBeat(values.duration)};
      if(draft.pitch===null||value(draft.duration)<=0)throw Error("Pitch and positive duration are required");
      emit({kind:lane?"drum_lane":"drum_add_lane",revision:formRevision,occurrence:source,...(lane?{from_pitch:lane.pitch,from_target:lane.target}:{}),...draft});
    });
  }
  function renderDrum(){
    const container=$("tracker-grid"),phrase=model.phrase.value;
    const key=stringify([model.revision,model.phrase.id,model.rowStart,model.drumLanes]);
    if(key===drumRenderKey){updateDrumPlayback();updateDrumCursor();return;}
    drumRenderKey=key;drumCursorKey=null;
    const left=container.scrollLeft,top=container.scrollTop,start=model.rowStart,end=start+model.rows.length;
    const table=document.createElement("table");table.className="tracker-table drum-grid";
    table.style.cssText=`min-width:${220+(end-start)*42}px;width:100%;table-layout:fixed`;
    table.setAttribute("aria-colcount",String(phrase.steps+1));
    const head=document.createElement("thead"),headRow=document.createElement("tr"),pitchLabel=document.createElement("th");
    pitchLabel.textContent="PITCH / OUTPUT";pitchLabel.style.width="220px";headRow.append(pitchLabel);
    for(let step=start;step<end;step++){
      const cell=document.createElement("th");cell.textContent=String(step+1);cell.style.width="40px";
      cell.dataset.step=String(step);cell.setAttribute("aria-colindex",String(step+2));headRow.append(cell);
    }
    head.append(headRow);table.append(head);
    const body=document.createElement("tbody"),offgrid=document.createElement("div");offgrid.className="drum-offgrid";
    offgrid.style.cssText="display:flex;gap:12px;flex-wrap:wrap;padding:12px 0";
    for(const lane of drumLanes()){
      const window=drumWindow(lane.events,gridBeat(phrase),{start,end});
      body.append(drumRow(lane,window));
      appendDrumOffgrid(offgrid,window.offgrid);
    }
    table.append(body);
    const hint=document.createElement("p");hint.className="capture-detail";
    hint.textContent="Left / Right: step · Up / Down: lane · Confirm: toggle · Clear: remove hit · Edit + Left / Right: velocity · Edit + Up: hit / lane settings. Right-click a hit for note, target, velocity and FX. Off-grid notes retain raw timing.";
    container.replaceChildren(table,hint,offgrid);container.scrollLeft=left;container.scrollTop=top;
    updateDrumPlayback();updateDrumCursor();
  }
  function drumRow(lane,window){
    const row=document.createElement("tr"),label=document.createElement("td");
    const drumNames={36:"KICK",38:"SNARE",42:"CLOSED HH",46:"OPEN HH"};
    const name=button((drumNames[lane.pitch]||"DRUM")+" / "+noteName(lane.pitch),()=>drumLaneDialog(lane));
    name.className="drum-lane";name.title=targetLabel(lane.target);
    const destination=document.createElement("small");destination.textContent=targetLabel(lane.target);name.append(destination);
    name.setAttribute("aria-label",`${noteName(lane.pitch)} lane · ${targetLabel(lane.target)}`);label.append(name);row.append(label);
    for(let step=window.start;step<window.end;step++)row.append(drumCell(lane,step,window.steps.get(step)||[]));
    return row;
  }
  function drumCell(lane,step,matches){
    const cell=document.createElement("td"),control=button(matches.length>1?String(matches.length):matches.length?"●":"·",()=>emit({kind:"drum_toggle",revision:model.revision,occurrence:model.occurrence,step,pitch:lane.pitch,target:lane.target,velocity:lane.velocity,duration:lane.duration}));
    cell.setAttribute("aria-colindex",String(step+2));
    control.className="drum-hit"+(matches.length?" active":"");control.dataset.step=String(step);control.dataset.lane=String(lane.index);
    control.dataset.hits=stringify(matches.map(event=>event.id));
    control.dataset.label=`${noteName(lane.pitch)}, step ${step+1}, ${matches.length?"on":"off"} · ${targetLabel(lane.target)}`;
    control.setAttribute("aria-pressed",String(matches.length>0));
    control.oncontextmenu=event=>{event.preventDefault();if(matches.length)noteDialog(step,matches[0]);};
    cell.append(control);return cell;
  }
  function appendDrumOffgrid(container,events){
    for(const event of events){
      const label=`${noteName(event.note.pitch)} @ ${beatLabel(event.at)} · ${targetLabel(event.target)}`;
      container.append(button(label,()=>noteDialog(value(event.at)/grid(model.phrase.value),event)));
    }
  }
  function updateDrumPlayback(){
    for(const control of $("tracker-grid").querySelectorAll(".drum-hit")){
      const playing=model.playing&&model.playRow===Number(control.dataset.step);
      control.classList.toggle("playing",playing);control.setAttribute("aria-label",control.dataset.label+(playing?" · Playing":""));
    }
  }
  function updateDrumCursor(){
    const key=stringify([model.row,model.column]);if(drumCursorKey===key)return;drumCursorKey=key;
    for(const cell of $("tracker-grid").querySelectorAll(".drum-grid th[data-step]")){
      const focused=Number(cell.dataset.step)===model.row;cell.classList.toggle("focused",focused);
    }
    for(const cell of $("tracker-grid").querySelectorAll(".drum-hit")){
      const focused=Number(cell.dataset.step)===model.row&&Number(cell.dataset.lane)===model.column;
      cell.classList.toggle("focused",focused);cell.setAttribute("aria-current",focused?"true":"false");
      if(focused)revealGridFocus(cell);
    }
  }
  function recordSetup(){
    if(!model.phrase)return;
    const source={revision:model.revision,occurrence:model.occurrence},active=model.capture?.target,profile=model.preferences?.mpe_input;
    const shared=(model.sharedReferences?.references.length||0)>1;
    const destinations=[...Object.entries(model.instruments||{}).map(([id,item])=>({value:"instrument:"+id,label:recordingInstrumentLabel(id,item)})),...(model.preferences?.mpe_outputs||[]).filter(profile=>(model.outputs?.connected||[]).includes(profile.endpoint)).map(profile=>({value:"external:"+encodeURIComponent(profile.endpoint),label:"EXTERNAL MPE · "+((model.outputs?.endpoints||[]).find(item=>item.id===profile.endpoint)?.name||profile.endpoint)}))];
    form("RECORD EXTERNAL MIDI · JAM WITH THE SONG",[
      {name:"mpe",label:"MIDI MODE",value:String(!!model.capture?.mpe),options:[{value:"false",label:"ORDINARY CHANNEL MIDI"},{value:"true",label:profile?mpeZoneLabel(profile.zone):"MPE · CONFIGURE SOURCE IN SETTINGS"}]},
      {name:"destination",label:"DESTINATION",value:active?targetChoice(active):destinations[0]?.value,options:destinations},
      {name:"monitor",label:"MONITOR",value:model.preferences?.monitor??false,options:[{value:"true",label:"ON · Hear input once"},{value:"false",label:"OFF"}]},
      ...(shared?[{name:"unique",label:"SHARED PHRASE",value:"true",options:[{value:"true",label:"Make unique here before arming"},{value:"false",label:"Record into shared phrase"}]}]:[])
    ],values=>{
      if(model.revision!==source.revision||!sameOccurrence(model.occurrence,source.occurrence))throw Error("Phrase changed; reopen Record Setup before arming.");
      if(!model.preferences?.active_input)throw Error("Choose a MIDI input using the INPUT row before arming.");
      if(!values.destination)throw Error("Add an instrument or connect a configured MPE output");
      const [kind,id]=values.destination.split(":"),external=kind==="external";
      emit({kind:"arm",mpe:external||values.mpe==="true",instrument:external?null:unsignedInteger(id),endpoint:external?decodeURIComponent(id):null,monitor:values.monitor==="true",unique:values.unique==="true"});
    });
    const fields=$("tracker-fields"),input=model.preferences?.active_input;
    fields.querySelector(".controller-hint").remove();
    const inputRow=setting("INPUT",input?.name||"CHOOSE MIDI INPUT",()=>{hideDialog();goTo("devices");});
    inputRow.querySelector("button").dataset.recordInput="true";fields.prepend(inputRow);
    const loop=document.createElement("div");loop.className="setting-row";
    const label=document.createElement("span"),detail=document.createElement("span");label.textContent="CAPTURE";detail.textContent=`RAW · ${beatLabel(gridBeat(model.phrase.value,model.phrase.value.steps))} BEAT LOOP · OVERDUB`;loop.append(label,detail);fields.append(loop);
    const arm=setting("ARM THIS PHRASE",`TRACK ${model.track+1} / PHRASE ${model.phrase.id} · RECORD AT LOOP START`,()=>$("tracker-form").requestSubmit());
    arm.querySelector("button").setAttribute("aria-label",`ARM THIS PHRASE · TRACK ${model.track+1} / PHRASE ${model.phrase.id}`);fields.append(arm);
    dialogNote(profile?`Declared source: ${profile.source}. Bend ranges: master ±${bendRangeLabel(profile.master_range)} / members ±${bendRangeLabel(profile.member_range)} semitones. Destination capability is verified when arming.`:"Ordinary MIDI preserves channel-wide controls. MPE requires an exact source profile in Tracker Settings; destination capability is verified when arming.");
    dialogNote("Arm pins this phrase occurrence. Record starts at its loop boundary; other tracks keep playing. Input and sound settings stay in Instruments.");
    $("tracker-dialog").classList.add("piano-setup");showPianoSheet("RECORD SETUP",`TRACK ${model.track+1} · PHRASE ${model.phrase.id} · ${input?"READY TO ARM":"CHOOSE INPUT"}`);
    $("tracker-form").querySelector("footer").hidden=true;
    updateRecordSetupInput();
    focusController(fields.querySelector("button"));
  }
  function updateRecordSetupInput(){
    const control=$("tracker-dialog").querySelector("[data-record-input]");
    if(!control)return;
    const input=model.preferences?.active_input,count=model.capture?.inputActivity;
    control.textContent=input?input.name+(count===null||count===undefined?" · WAITING FOR INPUT":` · ${count} ACCEPTED`):"CHOOSE MIDI INPUT";
  }
  function mpeZoneLabel(zone){
    const lower=zone.kind==="lower",channel=value=>String(value).padStart(2,"0");
    return `MPE ${zone.kind.toUpperCase()} · MASTER ${lower?"01":"16"} · MEMBERS ${channel(lower?2:16-zone.members)}–${channel(lower?zone.members+1:15)}`;
  }
  function bendRangeLabel(range){return String(range.semitones)+(range.cents?"."+String(range.cents).padStart(2,"0"):"");}
  function recordingInstrumentLabel(id,instrument){
    const capability=model.capture?.destinations?.find(item=>String(item.instrument)===String(id));
    const status=capability?capability.mpe_error===null?"MPE READY":"MPE UNAVAILABLE":"MPE NOT VERIFIED";
    return `INSTRUMENT ${String(id).padStart(2,"0")} / ${instrument.name} · ${status}`;
  }
  function pianoMetrics(phrase,zoom=pianoZoom){
    return new PianoTimeWindow({total:gridBeat(phrase,phrase.steps),anchor:pianoAnchor,viewportWidth:$("tracker-grid").clientWidth||1232,zoom});
  }
  function changePianoZoom(direction){const index=pianoZoomLevels.indexOf(pianoZoom);setPianoZoom(pianoZoomLevels[Math.max(0,Math.min(pianoZoomLevels.length-1,index+direction))]);}
  function setPianoZoom(zoom){
    if(!pianoZoomLevels.includes(zoom)||!model.phrase||model.phrase.value.form.kind!=="piano_roll")return;
    const container=$("tracker-grid"),phrase=model.phrase.value,old=pianoMetrics(phrase),viewport=container.clientWidth||1100;
    const cursorX=old.x(model.pianoControls.at)-container.scrollLeft;
    const cursorVisible=old.contains(model.pianoControls.at)&&cursorX>=PIANO_LABEL_WIDTH_PX&&cursorX<=viewport;
    const screenX=cursorVisible?cursorX:viewport/2,anchorPixels=container.scrollLeft+screenX-PIANO_LABEL_WIDTH_PX;
    if(zoom!==pianoZoom)pianoFollow=false;
    pianoZoom=zoom;const next=pianoMetrics(phrase),left=Math.max(0,Math.min(Math.max(0,next.width+PIANO_LABEL_WIDTH_PX-viewport),PIANO_LABEL_WIDTH_PX+anchorPixels*next.px/old.px-screenX));
    renderActions();renderPiano({scrollLeft:left,follow:false});
  }
  function renderPianoWindow(metrics){
    const node=$("tracker-grid-window"),key=stringify(["piano",model.phrase.id,metrics.start,metrics.end,metrics.total]);
    node.hidden=false;if(key===gridWindowKey)return;gridWindowKey=key;
    const label=document.createElement("span");label.textContent=`BEATS ${beatLabel(metrics.start)}–${beatLabel(metrics.end)} OF ${beatLabel(metrics.total)}`;
    const page=direction=>{pianoAnchor=metrics.adjacentSectionAnchor(direction);pianoFollow=false;renderActions();renderPiano({scrollLeft:0,follow:false});};
    const previous=button("PREVIOUS BEATS",()=>page(-1));previous.disabled=metrics.section===0n;
    const next=button("NEXT BEATS",()=>page(1));next.disabled=compareBeats(metrics.end,metrics.total)===0;
    const jump=button("GO TO BEAT",()=>form("Go to beat",[{name:"at",label:"Exact quarter-note beat",value:beatLabel(model.pianoControls.at)}],values=>{
      const at=parseBeat(values.at);if(compareBeats(at,metrics.total)>=0)throw Error("Choose a beat before the phrase end");
      pianoAnchor=at;pianoFollow=false;lastPianoCursor=null;
      emit({kind:"piano_control",control:{kind:"cursor",at,pitch:model.pianoControls.pitch}});
    }));
    node.replaceChildren(label,previous,next,jump);
  }
  function pianoContentSource(phrase){
    const take=model.capture?.draft,draft=take?.phrase===model.phrase.id?take.events:[],draftKey=stringify(draft);
    if(pianoSource?.phrase===phrase&&pianoSource.draftKey===draftKey)return pianoSource;
    const events=phrase.events.concat(draft);
    pianoSource={phrase,draftKey,storedEvents:new Set(phrase.events),summary:pianoSummary(events,model.pianoControls.pitch),index:new PianoEventIndex(events)};
    return pianoSource;
  }
  function togglePianoFollow(){pianoFollow=!pianoFollow;pianoFollowPosition=null;renderActions();renderPiano();}
  function pianoPlaybackBeat(phrase){
    if(!model.playing||!sameOccurrence(model.playOccurrence,model.occurrence))return null;
    const at=model.playPhraseBeat??(Number.isInteger(model.playRow)?gridBeat(phrase,model.playRow):null);
    return at&&compareBeats(at,gridBeat(phrase,phrase.steps))<0?at:null;
  }
  function followPianoPlayback(container,metrics,at,enabled){
    const key=at===null?null:stringify([model.playOccurrence,at]);
    const moved=key!==pianoFollowPosition;pianoFollowPosition=key;
    if(!enabled||!pianoFollow||at===null||!moved)return;
    const viewport=container.clientWidth||1100,margin=Math.min(96,viewport/5),head=metrics.x(at),left=container.scrollLeft;
    const next=head<left+margin?head-margin:head>left+viewport-margin?head-viewport+margin:left;
    container.scrollLeft=Math.max(0,Math.min(Math.max(0,metrics.width+PIANO_LABEL_WIDTH_PX-viewport),next));
  }
  let pianoExpressionKind="bend",pianoExpressionScope="expression";
  function renderOwnedExpression(event,metrics){
    const {width}=metrics;
    const node=document.createElement("section");node.className="expression-lane owned-expression";node.style.width=(width+PIANO_LABEL_WIDTH_PX)+"px";
    const toolbar=document.createElement("div");toolbar.className="expression-tools";
    for(const kind of ["bend","pressure","timbre","velocity","sustain"]){const choice=button(kind.toUpperCase()+(kind===pianoExpressionKind?" [SELECTED]":""),()=>{pianoExpressionKind=kind;renderPiano({follow:false});});choice.setAttribute("aria-pressed",String(kind===pianoExpressionKind));toolbar.append(choice);}
    const scope=event.note.mpe?pianoExpressionScope:"expression";
    if(event.note.mpe)for(const [key,label] of [["expression","MEMBER"],["master","MASTER"]]){const choice=button(label,()=>{pianoExpressionScope=key;renderPiano({follow:false});});choice.setAttribute("aria-pressed",String(scope===key));toolbar.append(choice);}
    node.append(toolbar);
    if(pianoExpressionKind==="velocity"){const text=document.createElement("p");text.textContent=`Attack ${event.note.velocity} / 127 · Release ${event.note.release_velocity} / 127`;node.append(text);toolbar.append(button("EDIT NOTE",()=>noteDialog(value(event.at)/grid(model.phrase.value),event)));return node;}
    const amount=control=>expressionLaneValue(control,pianoExpressionKind);
    const curve=scope==="master"?event.note.master_expression||[]:event.note.expression||[];
    const points=pianoExpressionPoints(event,curve,metrics,amount);
    const label=`OWNED ${scope==="master"?"MASTER":"MEMBER"} ${pianoExpressionKind.toUpperCase()} · ${curve.filter(point=>amount(point.control)!==null).length} raw points`;
    toolbar.append(button("EDIT "+(scope==="master"?"MASTER":"MEMBER")+" RAW POINTS",()=>performancePointDialog(event,scope,{revision:model.revision,phrase:model.phrase.id},model.phrase.value)));
    const svg=document.createElementNS("http://www.w3.org/2000/svg","svg");svg.setAttribute("width",String(width+PIANO_LABEL_WIDTH_PX));svg.setAttribute("height","64");svg.setAttribute("role","img");svg.setAttribute("aria-label",label+"; held values between recorded samples");
    const baseline=document.createElementNS(svg.namespaceURI,"path");baseline.setAttribute("d",`M 12 ${pianoExpressionKind==="bend"?32:60} H ${width+52}`);baseline.setAttribute("stroke","var(--color-border-strong)");svg.append(baseline);
    if(points.length){const maximum=pianoExpressionKind==="bend"?16383:127,path=document.createElementNS(svg.namespaceURI,"path"),x=point=>point.x,y=point=>60-point.value/maximum*56;let d=`M ${x(points[0])} ${y(points[0])}`;for(const point of points.slice(1))d+=` H ${x(point)} V ${y(point)}`;d+=` H ${Math.max(PIANO_LABEL_WIDTH_PX,Math.min(width+PIANO_LABEL_WIDTH_PX,metrics.offsetX(event.at,event.note.expression_duration??event.note.duration)))}`;path.setAttribute("d",d);path.setAttribute("fill","none");path.setAttribute("stroke","var(--color-accent-warning)");path.setAttribute("stroke-width","2");if(scope==="master")path.setAttribute("stroke-dasharray","5 3");svg.append(path);}
    node.append(svg);
    return node;
  }
  function renderPiano(options={}){
    const container=$("tracker-grid"),left=options.scrollLeft??container.scrollLeft,top=container.scrollTop,p=model.phrase.value;
    const playAt=pianoPlaybackBeat(p),cursorKey=stringify([model.phrase.id,model.pianoControls.at,model.pianoControls.pitch]);
    if(pianoSectionPhrase!==model.phrase.id){pianoSectionPhrase=model.phrase.id;pianoAnchor=model.pianoControls.at;lastPianoCursor=null;}
    let metrics=pianoMetrics(p);
    if(cursorKey!==lastPianoCursor&&!metrics.contains(model.pianoControls.at))pianoAnchor=model.pianoControls.at;
    else if(options.follow!==false&&pianoFollow&&playAt&&!metrics.contains(playAt))pianoAnchor=playAt;
    metrics=pianoMetrics(p);
    renderPianoWindow(metrics);
    updatePianoCapture();
    // Native editor revisions advance across Open/New as well as edits. Cursor
    // and playback updates need not stringify every stored note to detect edits.
    const key=stringify([model.phrase.id,model.revision,model.pianoControls.grid,model.pitchIntervals,model.scale,metrics.px,pianoZoom,pianoExpressionKind,pianoExpressionScope,model.playing,model.capture?.state,model.capture?.draft?.events,model.capture?.held]);
    const previousRoll=container.querySelector(".piano-roll"),sameContent=key===pianoRenderKey;
    const pitch=model.pianoControls.pitch;
    if(sameContent&&previousRoll&&pitch>=Number(previousRoll.dataset.low)&&pitch<=Number(previousRoll.dataset.high)){
      if(compareBeats(pianoViewport.metrics.start,metrics.start)!==0)updatePianoTimeWindow(pianoViewport,metrics);
      updatePianoSelection(pianoViewport);
      const head=container.querySelector(".piano-playhead");if(head){head.hidden=playAt===null||!metrics.contains(playAt);if(playAt!==null)head.style.left=metrics.x(playAt)+"px";}
      container.scrollLeft=left;followPianoPlayback(container,metrics,playAt,options.follow!==false);updatePianoCursor(container,previousRoll,metrics);updatePianoViewport();return;
    }
    pianoRenderKey=key;
    container.style.setProperty("--piano-viewport-width",container.clientWidth+"px");
    const source=pianoContentSource(p),{summary}=source,selected=new Set(model.selectedEvents||[]);
    // Keep pitch geometry stable during navigation. Extend an octave at the
    // edge instead of rebuilding every note for each additional semitone.
    const low=sameContent&&previousRoll?Math.max(0,Math.min(Number(previousRoll.dataset.low),pitch-12)):Math.min(summary.low,pitch);
    const high=sameContent&&previousRoll?Math.min(127,Math.max(Number(previousRoll.dataset.high),pitch+12)):Math.max(summary.high,pitch);
    const {content,view}=createPianoViewport(p,source,{metrics,low,high,selected,playAt,previousList:container.querySelector(".piano-event-list"),viewportWidth:container.clientWidth});
    pianoViewport=view;
    container.replaceChildren(content);updatePianoSelection(pianoViewport);container.scrollLeft=left;container.scrollTop=top;updatePianoCapture();followPianoPlayback(container,metrics,playAt,options.follow!==false);
    updatePianoCursor(container,view.roll,metrics);updatePianoViewport();
  }
  function createPianoViewport(p,source,layout){
    const {metrics,low,high,selected,playAt,previousList,viewportWidth}=layout;
    const {total,width}=metrics,{storedEvents,index,summary:{notes}}=source;
    const roll=document.createElement("div");roll.className="piano-roll";roll.style.width=(width+PIANO_LABEL_WIDTH_PX)+"px";roll.style.height=((high-low+1)*22+36)+"px";
    roll.dataset.low=String(low);roll.dataset.high=String(high);
    updatePianoTicks(roll,metrics);
    // Notes and the edit cursor carry exact time/pitch labels. Repeating every
    // decorative axis label floods native accessibility on tall/long phrases.
    for(let pitch=high;pitch>=low;pitch--){const row=document.createElement("div");row.className="piano-pitch"+([1,3,6,8,10].includes(pitch%12)?" accidental":"");row.style.top=(36+(high-pitch)*22)+"px";row.setAttribute("aria-hidden","true");
      const name=document.createElement("span");name.textContent=model.pitchLabels?.[pitch]||noteName(pitch);row.append(name);row.onclick=e=>pianoPointerAction(e,row,pitch,false);row.ondblclick=e=>pianoPointerAction(e,row,pitch,true);roll.append(row);}
    const noteLayer=document.createElement("div");noteLayer.className="piano-notes";roll.append(noteLayer);
    const cursor=document.createElement("div");cursor.className="piano-cursor";cursor.style.cssText="position:absolute;top:36px;bottom:0;border-left:1px dashed var(--color-accent-warning);pointer-events:none;z-index:4";
    const cursorPoint=document.createElement("span");cursorPoint.className="piano-cursor-point";
    cursorPoint.style.cssText="position:absolute;left:0;width:7px;height:20px;border:2px solid var(--color-accent-warning);box-sizing:border-box";
    cursorPoint.setAttribute("role","img");
    cursor.append(cursorPoint);roll.append(cursor);
    const head=document.createElement("div");head.className="piano-playhead";head.hidden=playAt===null||!metrics.contains(playAt);head.style.left=(playAt?metrics.x(playAt):PIANO_LABEL_WIDTH_PX)+"px";roll.append(head);
    const expression=document.createElement("div");expression.className="expression-lane";expression.style.width=(width+PIANO_LABEL_WIDTH_PX)+"px";const label=document.createElement("p");label.textContent="RAW CHANNEL EXPRESSION · BEND / PRESSURE / CC / SUSTAIN";expression.append(label);
    const controlLayer=document.createElement("div");controlLayer.className="piano-controls";expression.append(controlLayer);
    const detail=document.createElement("p");detail.className="capture-detail";
    const eventList=renderPianoEventList(previousList);
    const routing=document.createElement("p");routing.className="piano-routing";
    const noteDetail=document.createElement("p");noteDetail.className="piano-note-detail";
    const content=document.createElement("div");content.className="piano-content";content.style.width=Math.max(width+PIANO_LABEL_WIDTH_PX,viewportWidth)+"px";
    content.append(routing,roll,expression,noteDetail,detail,eventList);
    const view={roll,noteLayer,controlLayer,rawControlLayer:controlLayer,channelExpression:expression,expressionSurface:expression,noteDetail,detail,eventList,noteCount:notes.length,selectionKey:null,metrics,index,storedEvents,selected,phrase:p,high,total,noteNodes:new Map(),noteGroups:new Map(),controlNodes:new Map()};
    return {content,view};
  }
  function updatePianoTicks(roll,metrics){
    const ticks=Array.from(roll.querySelectorAll(".piano-tick")),markers=metrics.ticks(model.pianoControls.grid);
    for(const [index,marker] of markers.entries()){
      let tick=ticks[index];
      if(!tick){tick=document.createElement("span");tick.className="piano-tick";tick.setAttribute("aria-hidden","true");roll.append(tick);}
      const left=marker.left+"px";if(tick.style.left!==left)tick.style.left=left;
      const label=marker.label?beatLabel(marker.at):"";if(tick.textContent!==label)tick.textContent=label;
    }
    for(const tick of ticks.slice(markers.length))tick.remove();
  }
  function updatePianoTimeWindow(view,metrics){
    view.metrics=metrics;view.visibleBounds=null;view.selectionKey=null;
    view.roll.style.width=(metrics.width+PIANO_LABEL_WIDTH_PX)+"px";
    view.roll.parentElement.style.width=Math.max(metrics.width+PIANO_LABEL_WIDTH_PX,$("tracker-grid").clientWidth)+"px";
    view.channelExpression.style.width=(metrics.width+PIANO_LABEL_WIDTH_PX)+"px";
    updatePianoTicks(view.roll,metrics);
    // A section changes note coordinates and membership. Keep the pitch grid,
    // cursor, routing and event list; rematerialize only this section's controls.
    view.noteLayer.replaceChildren();view.noteNodes.clear();view.noteGroups.clear();
    view.rawControlLayer.replaceChildren();view.controlNodes.clear();
  }
  function pianoPointerAction(event,row,pitch,insert){
    try{
      const at=pianoViewport.metrics.editGridBeatAtPixel(event.clientX-row.getBoundingClientRect().left,model.pianoControls.grid);
      if(insert)noteDialog(model.row,null,noteName(pitch),true,at);
      else{clearEventSelection();emit({kind:"piano_control",control:{kind:"cursor",at,pitch}});}
    }catch(error){$("tracker-status").textContent=error.message;}
  }
  function updatePianoSelection(view){
    const key=stringify(model.selectedEvents||[]);if(key===view.selectionKey)return;
    view.selectionKey=key;view.selected=new Set(model.selectedEvents||[]);
    // Keep the actual hit targets between the first and second click. Replacing
    // the roll on selection swallows double-clicks and keyboard focus.
    for(const [event,node] of view.noteNodes)setPianoNoteSelected(node,view.storedEvents.has(event)&&view.selected.has(event.id));
    for(const [event,node] of view.controlNodes)node.setAttribute("aria-pressed",String(view.storedEvents.has(event)&&view.selected.has(event.id)));
    const selectedNote=view.phrase.events.find(event=>view.selected.has(event.id)&&event.note);
    const next=selectedNote?renderOwnedExpression(selectedNote,view.metrics):view.channelExpression;
    if(next!==view.expressionSurface){view.expressionSurface.replaceWith(next);view.expressionSurface=next;}
    view.controlLayer=selectedNote?null:view.rawControlLayer;
    view.noteDetail.textContent=pianoNoteDetailText(selectedNote);
    const identities=new Set(Array.from(view.selected,String));
    for(const choice of view.eventList.querySelectorAll("button[data-event-id]")){
      const selected=identities.has(choice.dataset.eventId);choice.setAttribute("aria-pressed",String(selected));choice.className=selected?"selected":"muted";
    }
  }
  function pianoNoteDetailText(event){
    return event?`${noteName(event.note.pitch)} · Velocity ${event.note.velocity} / 127 · Release ${event.note.release_velocity} / 127 · ${targetLabel(event.target)} · key ${beatLabel(event.note.duration)} beats · expression ${beatLabel(event.note.expression_duration??event.note.duration)} beats${event.note.mpe?" · source "+event.note.mpe.profile.source+" · member "+event.note.mpe.member:""}`:"Select a note to inspect its owned expression. Raw timing stays intact.";
  }
  function setPianoNoteSelected(note,selected){
    note.classList.toggle("selected",selected);note.setAttribute("aria-pressed",String(selected));
    note.style.outline=selected?"2px solid var(--color-accent-focus)":"";note.style.outlineOffset=selected?"1px":"";
  }
  function updatePianoViewport(){
    const view=pianoViewport;if(!view?.roll.isConnected)return;
    const container=$("tracker-grid"),clip=container.getBoundingClientRect(),roll=view.roll.getBoundingClientRect();
    // Overscan avoids edge flicker while scrolling; source length never limits
    // authored content. Only the DOM outside this pixel rectangle is omitted.
    const bounds={left:container.scrollLeft-96,right:container.scrollLeft+container.clientWidth+96,
      high:view.high-Math.floor((clip.top-roll.top-36-44)/22),
      low:view.high-Math.floor((clip.top+container.clientHeight-roll.top-36+44)/22)};
    const previous=view.visibleBounds,focus=document.activeElement;
    if(previous&&previous.controls===view.controlLayer&&previous.focus===focus&&Object.keys(bounds).every(key=>previous[key]===bounds[key]))return;
    view.visibleBounds={...bounds,controls:view.controlLayer,focus};
    syncPianoElements({layer:view.noteLayer,nodes:view.noteNodes,create:entry=>pianoNoteElement(entry,view),parentFor:entry=>ensurePianoNotePage(entry,view)},view.index.notesIn(bounds,view.metrics));
    for(const [pitch,group] of view.noteGroups){
      prunePianoNoteGroups(group);
      if(!group.pages.size){group.node.remove();view.noteGroups.delete(pitch);}
    }
    if(view.controlLayer)syncPianoElements({layer:view.controlLayer,nodes:view.controlNodes,create:entry=>pianoControlElement(entry,view)},view.index.controlsIn(bounds,view.metrics));
  }
  function prunePianoNoteGroups(group){
    for(const [page,node] of group.pages){if(!node.childElementCount){node.remove();group.pages.delete(page);}}
  }
  // Create missing pitch/page parents and return the bounded page for this note.
  function ensurePianoNotePage(entry,view){
    const pitch=entry.event.note.pitch;
    let group=view.noteGroups.get(pitch);
    if(!group){
      const node=document.createElement("div");node.setAttribute("role","group");
      node.setAttribute("aria-label",`${model.pitchLabels?.[pitch]||noteName(pitch)} notes`);
      group={node,pages:new Map()};view.noteGroups.set(pitch,group);view.noteLayer.append(node);
    }
    let page=group.pages.get(entry.group);
    if(!page){
      // WebKit's native accessibility truncates crowded parents even when
      // their children paint. Bound each group, never the source or viewport.
      page=document.createElement("div");page.setAttribute("role","group");
      page.setAttribute("aria-label",`Note group ${entry.group+1}`);
      group.pages.set(entry.group,page);group.node.append(page);
    }
    return page;
  }
  function syncPianoElements(target,entries){
    const {layer,nodes,create,parentFor=()=>layer}=target;
    const visible=new Set(entries.map(entry=>entry.event));
    for(const [event,node] of nodes){
      // Keep at most the keyboard-focused offscreen control until focus leaves.
      if(!visible.has(event)&&document.activeElement!==node){node.remove();nodes.delete(event);}
    }
    const previousByParent=new Map();
    for(const entry of entries){
      let node=nodes.get(entry.event);
      if(!node){node=create(entry);nodes.set(entry.event,node);}
      const parent=parentFor(entry),previous=previousByParent.get(parent);
      const next=previous?previous.nextSibling:parent.firstChild;
      if(next!==node)parent.insertBefore(node,next);
      previousByParent.set(parent,node);
    }
  }
  function pianoNoteElement(entry,view){
    const e=entry.event,stored=view.storedEvents.has(e),selected=stored&&view.selected.has(e.id);
    const note=button((model.pitchLabels?.[e.note.pitch]||noteName(e.note.pitch))+(view.metrics.offsetX(e.at,e.note.duration)>view.metrics.x(view.total)?" ›":""),click=>{if(stored)selectEvent(e.id,click.shiftKey||click.metaKey||click.ctrlKey);});
    note.className="piano-note"+(selected?" selected":"")+(!stored?" draft":"");
    if(stored)note.dataset.eventId=String(e.id);
    if(!(view.phrase.key||model.scale).pitch_classes.includes(e.note.pitch%12))note.classList.add("outside-scale");
    note.style.left=entry.left+"px";note.style.top=(36+(view.high-e.note.pitch)*22)+"px";note.style.width=entry.width+"px";
    setPianoNoteSelected(note,selected);if(!stored)note.style.opacity="0.55";
    note.setAttribute("aria-label",pianoNoteLabel(e,stored));note.title=note.getAttribute("aria-label");
    if(stored){note.ondblclick=()=>noteDialog(value(e.at)/grid(view.phrase),e);note.oncontextmenu=click=>{click.preventDefault();noteDialog(value(e.at)/grid(view.phrase),e);};}
    return note;
  }
  function pianoNoteLabel(event,stored){return `${eventLabel(event)}, duration ${beatLabel(event.note.duration)} beats, velocity ${event.note.velocity}, release velocity ${event.note.release_velocity}${stored?"":"; uncommitted capture"}`;}
  function pianoControlElement(entry,view){
    const event=entry.event,stored=view.storedEvents.has(event),item=button("│",click=>{if(stored)selectEvent(event.id,click.shiftKey||click.metaKey||click.ctrlKey);});
    item.className="expression-point";item.style.left=entry.left+"px";item.style.width=entry.width+"px";
    item.title=eventLabel(event)+" · "+event.controls.map(controlLabel).join("; ");item.setAttribute("aria-label",item.title);item.setAttribute("aria-pressed",String(stored&&view.selected.has(event.id)));
    if(stored)item.ondblclick=()=>performanceDialog(event);
    return item;
  }
  function updatePianoCursor(container,roll,metrics){
    const cursor=roll.querySelector(".piano-cursor"),point=cursor.firstElementChild,{at,pitch}=model.pianoControls;
    cursor.hidden=!metrics.contains(at);cursor.style.left=metrics.x(at)+"px";
    point.style.top=((Number(roll.dataset.high)-pitch)*22+1)+"px";
    point.dataset.at=beatKey(at);point.dataset.pitch=String(pitch);
    point.setAttribute("aria-label","Edit cursor · beat "+beatLabel(at)+" · "+noteName(pitch));
    const cursorKey=stringify([model.phrase.id,model.pianoControls.at,model.pianoControls.pitch]);
    if(cursorKey!==lastPianoCursor){lastPianoCursor=cursorKey;if(!cursor.hidden)revealPianoCursor(container,point);}
  }
  function pianoLabelsRight(roll){return roll.querySelector(".piano-pitch>span").getBoundingClientRect().right;}
  function revealPianoCursor(container,cursor){
    const point=cursor.getBoundingClientRect(),bounds=container.getBoundingClientRect();
    const clip={left:bounds.left+container.clientLeft,top:bounds.top+container.clientTop};
    clip.right=clip.left+container.clientWidth;clip.bottom=clip.top+container.clientHeight;
    clip.left=Math.max(clip.left,pianoLabelsRight(cursor.closest(".piano-roll")));
    if(point.left<clip.left)container.scrollLeft-=clip.left-point.left;
    else if(point.right>clip.right)container.scrollLeft+=point.right-clip.right;
    if(point.top<clip.top)container.scrollTop-=clip.top-point.top;
    else if(point.bottom>clip.bottom)container.scrollTop+=point.bottom-clip.bottom;
  }
  function renderPianoEventList(previous){
    const list=document.createElement("details");list.className="piano-event-list";
    list.dataset.phrase=String(model.phrase.id);
    const samePhrase=previous?.dataset.phrase===list.dataset.phrase;
    list.open=!!(samePhrase&&previous.open);list.dataset.start=samePhrase?previous.dataset.start:"0";
    list.setAttribute("aria-label","All phrase events, including overlapping notes and raw controls");
    const title=document.createElement("summary");title.textContent=`ALL PHRASE EVENTS (${model.phrase.value.events.length}) · OVERLAPS AND RAW CONTROLS`;
    const body=document.createElement("div");body.className="piano-event-page";list.append(title,body);
    list.ontoggle=()=>{if(list.isConnected)fillPianoEventPage(list);};
    if(list.open)fillPianoEventPage(list);
    return list;
  }
  function fillPianoEventPage(list){
    const body=list.querySelector(".piano-event-page");body.replaceChildren();
    if(!list.open||list.dataset.phrase!==String(model.phrase.id))return;
    const page=pageItems(model.phrase.value.events,Number(list.dataset.start)),selected=new Set(model.selectedEvents||[]);
    list.dataset.start=String(page.start);
    const nav=document.createElement("nav");nav.className="piano-event-pages";nav.setAttribute("aria-label","Phrase event pages");
    const move=offset=>{list.dataset.start=String(offset);fillPianoEventPage(list);body.querySelector("button:not(:disabled)")?.focus();};
    const previous=button("PREVIOUS EVENTS",()=>move(page.start-EVENT_PAGE_SIZE));previous.disabled=page.start===0;
    const next=button("NEXT EVENTS",()=>move(page.end));next.disabled=page.end>=page.total;
    const position=document.createElement("span");position.textContent=page.total?`EVENTS ${page.start+1}–${page.end} OF ${page.total}`:"NO EVENTS";position.setAttribute("role","status");
    const choose=button("CHOOSE NOTE / EVENT",()=>eventChooser("select",page.start));choose.disabled=page.total===0;
    nav.append(previous,next,choose,position);body.append(nav);
    for(const event of page.values)body.append(renderPianoEventRow(event,selected.has(event.id)));
  }
  function renderPianoEventRow(event,selected){
    const row=document.createElement("div");row.className="piano-event-row";
    const choose=button(eventLabel(event),click=>selectEvent(event.id,click.shiftKey||click.metaKey||click.ctrlKey));
    choose.dataset.eventId=String(event.id);
    choose.setAttribute("aria-pressed",String(selected));choose.className=selected?"selected":"muted";
    const points=(event.note?.expression.length||0)+(event.note?.master_expression?.length||0);
    row.append(choose,button("EDIT",()=>noteDialog(value(event.at)/grid(model.phrase.value),event)),button("RAW MIDI"+(points?` (${points})`:""),()=>performanceDialog(event)));
    return row;
  }
  function updatePianoCapture(){
    const state=model.capture?.state?.state||"idle",capture=model.capture||{},draft=capture.draft;
    const pass=draft&&model.playBeat?Math.max(1,1+Math.floor((value(model.playBeat)-value(draft.started_at))/value(draft.length))):null;
    const status=$("tracker-actions").querySelector(".piano-capture-state");
    const timing=["armed","recording","waiting"].includes(state)?`OVERDUB · MONITOR ${capture.monitor?"ON":"OFF"}`:"RAW TIMING";
    const phase=capture.ending?"ENDING TAKE":state==="armed"?"READY":pass&&state==="recording"?"PASS "+String(pass).padStart(2,"0"):state.toUpperCase();
    if(status)status.textContent=`${phase} · ${timing}`;
    const routing=$("tracker-grid").querySelector(".piano-routing");
    if(routing)routing.textContent=`IN  ${model.preferences?.active_input?.name||"No MIDI input selected"}${capture.mpe?" / "+capture.mpe.zone.kind.toUpperCase()+" MPE":""}    OUT  ${capture.target?targetLabel(capture.target):"Choose in Record setup"}    ${pianoCaptureLocation(capture)}`;
    const detail=$("tracker-grid").querySelector(".capture-detail");
    if(detail)detail.textContent=pianoCaptureHint(capture);
  }
  function pianoCaptureLocation(capture){
    const selected=`Track ${model.track+1} · Phrase ${model.phrase.id}`,armed=capture.armed;
    if(!armed)return "SELECTED  "+selected;
    const index=model.tracks.findIndex(track=>track.id===armed.occurrence.track);
    const track=index<0?`Track ID ${armed.occurrence.track}`:`Track ${index+1}`;
    const label=["armed","waiting","recording"].includes(capture.state?.state)?"ARMED":"TAKE";
    return `${label}  ${track} · Phrase ${armed.phrase}`+(sameOccurrence(armed.occurrence,model.occurrence)?"":`    SELECTED  ${selected}`);
  }
  function pianoCaptureHint(capture){
    if(capture.ending)return `Ending take at beat ${beatLabel(capture.ending)}. Held notes close at the loop boundary; recorded timing and expression stay intact.`;
    switch(capture.state?.state){
      case "armed":return "Armed is not recording. Record starts at the armed phrase boundary; other tracks keep playing. Other phrases remain editable.";
      case "waiting":return `Waiting for armed phrase start at beat ${beatLabel(capture.state.boundary)}. Release keys and pedals before recording.`;
      case "recording":return `Take is ${capture.monitor?"monitored now":"recording with monitoring off"}. End Take commits at the next loop boundary. ${capture.held||0} held · Other phrases remain editable; the armed phrase stays fixed.`;
      case "interrupted":return `Take interrupted: ${capture.state.reason}. ${capture.held||0} held · Commit or save as a new phrase; discard only when no longer needed.`;
      default:return capture.draft?"Take retained. Commit, save as a new phrase, or discard using More Actions.":"Click selects; Shift/Cmd/Ctrl-click toggles. Double-click or right-click edits. Raw timing retained.";
    }
  }
  function trackDialog(){const track=model.tracks[model.track];form("Track settings",[{name:"name",label:"Name",value:track.name},{name:"looped",label:"Loop",value:String(track.looped),options:[{value:"false",label:"Stop after last slot"},{value:"true",label:"Loop track"}]},{name:"muted",label:"Mute",value:String(track.muted),options:[{value:"false",label:"Playing"},{value:"true",label:"Muted"}]}],v=>edit({kind:"put_track",track:{...track,name:v.name,looped:v.looped==="true",muted:v.muted==="true"}}));}
  function arrangeDialog(){const track=model.tracks[model.track],current=track.slots[model.slot];form("Song slot "+(model.slot+1),[{name:"chain",label:"Chain",value:current?.chain??"new",options:[{value:"new",label:"Create new chain and phrase"},{value:"empty",label:"Empty (stops track)"},...Object.entries(model.chains).map(([value,c])=>({value,label:c.name}))]},{name:"repeats",label:"Repeats (existing chain)",value:current?.repeats??1,type:"number",min:1}],v=>{const slot=Math.min(model.slot,track.slots.length);if(v.chain==="new"){const commands=[];if(current){const empty=structuredClone(track);empty.slots[slot]=null;commands.push({kind:"put_track",track:empty});}commands.push({kind:"add_phrase",track:track.id,slot});edit({kind:"batch",commands});return;}if(v.chain==="empty"){request("clear");return;}const chain=unsignedInteger(v.chain),repeats=Number(v.repeats);if(repeats===1&&(current?.chain!==chain||current.repeats===1)){emit({kind:"assign_chain",track:track.id,slot,chain});return;}const next=structuredClone(track);next.slots[slot]={id:current?.chain===chain?current.id:model.nextIdentity,chain,repeats};edit({kind:"put_track",track:next});});}
  function chainDialog(){if(!model.selectedChain)return;form("Add phrase to chain",[{name:"phrase",label:"Phrase",value:"new",options:[{value:"new",label:"New phrase"},...Object.entries(model.phrases).map(([value,p])=>({value,label:p.name}))]}],v=>emit({kind:"append_phrase",phrase:v.phrase==="new"?null:unsignedInteger(v.phrase)}));}
  function chainPlacementDialog(row=model.chainRow){
    const selected=model.selectedChain;if(!selected)return;
    const chain=structuredClone(selected.value),placement=chain.phrases[row];
    if(!placement){chainDialog();return;}
    form("Phrase placement "+(row+1),[
      {name:"phrase",label:"Phrase reference",value:placement.phrase,options:Object.entries(model.phrases).map(([value,phrase])=>({value,label:`${value} / ${phrase.name}`}))},
      {name:"repeats",label:"Repeats at this placement",value:placement.repeats,type:"number",min:1,max:4294967295},
      {name:"position",label:"Position in chain",value:row+1,type:"number",min:1,max:chain.phrases.length},
      {name:"action",label:"Action",value:"keep",options:[{value:"keep",label:"Update placement"},{value:"remove",label:"Remove placement and close gap"}]}
    ],values=>{
      const position=Number(values.position)-1,repeats=Number(values.repeats);
      if(!Number.isInteger(position)||position<0||position>=chain.phrases.length||!Number.isInteger(repeats)||repeats<1||repeats>4294967295)throw Error("Choose a valid position and positive whole repeat count");
      const [usage]=chain.phrases.splice(row,1);
      if(values.action!=="remove")chain.phrases.splice(position,0,{...usage,phrase:unsignedInteger(values.phrase),repeats});
      edit({kind:"put_chain",id:selected.id,chain});
    });
    dialogNote("Placement identity follows the move. Phrase content stays linked. Shared chains ask before changing other uses; Undo restores the complete edit.");
  }
  function chainSettingsDialog(){
    const selected=model.selectedChain;if(!selected)return;
    const chain=structuredClone(selected.value);
    form("Chain settings",[{name:"name",label:"Chain name",value:chain.name}],values=>{chain.name=values.name;edit({kind:"put_chain",id:selected.id,chain});});
  }
  function makeChainUnique(){
    const selected=model.selectedChain,track=model.tracks[model.track],usage=track.slots[model.slot];
    if(!selected||!usage)return;
    edit({kind:"edit_unique",location:{track:track.id,chain_use:usage.id,phrase_use:null},command:{kind:"put_chain",id:selected.id,chain:structuredClone(selected.value)}});
  }
  function moveSlotDialog(){
    const track=structuredClone(model.tracks[model.track]),row=model.slot;
    if(row>=track.slots.length){arrangeDialog();return;}
    form("Move Song slot",[{name:"position",label:"Position in "+track.name,value:row+1,type:"number",min:1,max:track.slots.length}],values=>{
      const position=Number(values.position)-1;
      if(!Number.isInteger(position)||position<0||position>=track.slots.length)throw Error("Choose an existing position in this track");
      const [usage]=track.slots.splice(row,1);track.slots.splice(position,0,usage);
      edit({kind:"put_track",track});
    });
    dialogNote("Reorders this track without replacing any chain. Empty slots still stop the track. The selected occurrence and its View Group identity follow the move; Undo restores its previous position.");
  }
  function generatorDialog(){
    const phrase=model.phrase.value,recipe=phrase.form;
    const fields=recipe.kind==="euclidean"?[
      {name:"pulses",label:"Pulses",value:recipe.pulses,type:"number",min:0,max:phrase.steps},
      {name:"rotation",label:"Rotation",value:recipe.rotation,type:"number",min:0,max:phrase.steps-1},
      {name:"pitch",label:"Pitch",value:noteName(recipe.pitch)},
      {name:"velocity",label:"Attack velocity",value:recipe.velocity,type:"number",min:1,max:127}
    ]:[
      {name:"seed",label:"Seed (unsigned 64-bit integer)",value:String(recipe.seed)},
      {name:"density",label:"Density %",value:recipe.density,type:"number",min:0,max:100},
      {name:"low",label:"Lowest pitch",value:noteName(recipe.low)},
      {name:"high",label:"Highest pitch",value:noteName(recipe.high)},
      {name:"velocity_low",label:"Minimum velocity",value:recipe.velocity_low,type:"number",min:1,max:127},
      {name:"velocity_high",label:"Maximum velocity",value:recipe.velocity_high,type:"number",min:1,max:127}
    ];
    if(recipe.kind==="random")fields.push({name:"gate_high",label:"Maximum gate (step lengths)",value:beatLabel(recipe.gate_high||recipe.gate)});
    fields.push({name:"gate",label:recipe.kind==="random"?"Minimum gate (step lengths)":"Gate (step lengths)",value:beatLabel(recipe.gate)},
      {name:"target",label:"Instrument / output",value:targetChoice(recipe.target),options:soundTargets()},
      {name:"channel",label:"Output channel",value:recipe.target.channel,type:"number",min:1,max:16});
    form("Generator recipe · first-cycle preview",fields,values=>{
      const updated=structuredClone(phrase);
      for(const [key,text] of Object.entries(values)){
        if(key==="target"||key==="channel")continue;
        updated.form[key]=["pitch","low","high"].includes(key)?parseNote(text):["gate","gate_high"].includes(key)?parseBeat(text):key==="seed"?unsignedInteger(text):Number(text);
      }
      updated.form.target=chosenTarget(values.target,values.channel);
      edit({kind:"put_phrase",id:model.phrase.id,phrase:updated});
    });
  }

  document.addEventListener("keydown",event=>{
    if($("tracker-dialog").open){
      if(event.shiftKey&&event.key==="ArrowUp"&&$("tracker-dialog").dataset.songFile==="true"){
        event.preventDefault();event.stopPropagation();controllerAction("shift_up");return;
      }
      if(document.activeElement?.tagName==="BUTTON"&&["ArrowUp","ArrowDown"].includes(event.key)){
        event.preventDefault();event.stopPropagation();controllerAction(event.key==="ArrowUp"?"previous":"next");
      }
      return;
    }
    if(!controllerSurface||!model.controlsFocused)return;
    const actions={ArrowUp:"previous",w:"previous",ArrowDown:"next",s:"next",ArrowLeft:"decrease",a:"decrease",ArrowRight:"increase",d:"increase",Enter:"confirm",Escape:"back",q:"back"};
    const action=actions[event.key];if(!action)return;
    event.preventDefault();event.stopPropagation();controllerAction(action);
  });
  document.addEventListener("keydown",event=>{
    if(model?.view!=="phrase"||model.phrase?.value.form.kind!=="piano_roll"||$("tracker-dialog").open||["INPUT","TEXTAREA","SELECT"].includes(document.activeElement?.tagName)||document.activeElement?.isContentEditable)return;
    const key=event.key.toLowerCase(),modifier=event.metaKey||event.ctrlKey;
    let action=null;
    if(modifier&&key==="a")action=selectAllEvents;
    else if(modifier&&["c","x"].includes(key))action=()=>selectionRequest({kind:key==="c"?"copy":"cut"});
    else if(modifier&&key==="v")action=()=>selectionDialog("paste");
    else if(!modifier&&["delete","backspace"].includes(key)&&model.selectedEvents?.length)action=()=>selectionRequest({kind:"delete"});
    else if(key==="escape")action=clearEventSelection;
    if(action){event.preventDefault();event.stopPropagation();action();}
  });
  window.addEventListener("resize",()=>{if(model?.view==="song"){renderActions();renderGrid();updateSongPlayback();}if(model?.view==="phrase"&&model.phrase?.value.form.kind==="piano_roll"){pianoRenderKey=null;renderPiano({follow:false});}});
  $("tracker-grid").addEventListener("scroll",()=>{
    if(!pianoViewport||pianoViewportFrame!==null)return;
    pianoViewportFrame=requestAnimationFrame(()=>{pianoViewportFrame=null;updatePianoViewport();});
  },{passive:true});
  document.querySelectorAll("[data-view]").forEach(node=>node.onclick=()=>emit({kind:"view",view:node.dataset.view}));
  document.querySelectorAll("[data-request]").forEach(node=>node.onclick=()=>request(node.dataset.request));
  $("tracker-dialog-close").onclick=closeDialog;$("tracker-dialog-cancel").onclick=closeDialog;
  $("tracker-dialog").oncancel=event=>{event.preventDefault();closeDialog();};
  $("tracker-dialog").onclose=()=>{if($("tracker-dialog").open)return;cancelFileDialog();closeCharacterChooser(false);clearControllerFocus();formRevision=null;emit({kind:"text_entry",active:false});};
  const phraseReceiver=new CrestTrackerProjection.PhraseReceiver();
  window.__TAURI__.event.listen("crest://host-projection",event=>{
    const received=phraseReceiver.receive(decode(event.payload));
    if(!received.model){if(received.resync)request("phrase_projection_resync");return;}
    render(received.model);acknowledgeFeedback(received.model);
    if(received.acknowledge)emit({kind:"phrase_projection_ack",token:received.acknowledge});
  }).then(()=>request("phrase_projection_resync"));
})();
