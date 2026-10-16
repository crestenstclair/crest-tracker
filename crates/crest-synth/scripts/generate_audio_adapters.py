#!/usr/bin/env python3
"""Generate source-isolation wrappers; never generate or modify DSP algorithms."""
from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from vendor_audio_sources import AIRWINDOWS, MDA
root=Path(__file__).resolve().parents[1]
out=root/'native/audio'
entries=[]
for family,names in [('airwindows',AIRWINDOWS),('mda',MDA)]:
 for name in names:
  cls=name if family=='airwindows' else 'mda'+name
  base=f'vendor/audio/airwindows/plugins/WinVST/{name}/{name}' if family=='airwindows' else f'vendor/audio/mda/plugins/{cls}'
  if not (root/(base+'.cpp')).exists():continue
  symbol=family+'_'+name
  instrument=family=='mda' and name in ('JX10','Piano','EPiano')
  text='#include "plugin_adapter.h"\nnamespace '+symbol+' {\n'
  # The upstream Piano diagnostic must never reach the callback. Formatting
  # functions used for metadata remain available off callback.
  if name=='Piano':text+='#define printf(...) ((void)0)\n'
  text+=('#include "crest_mda_epiano.cpp"\n' if name=='EPiano' else f'#include "../../{base}.cpp"\n')
  if family=='airwindows':text+=f'#include "../../{base}Proc.cpp"\n'
  text+='}\n'
  if name=='Piano':text+='#undef printf\n'
  text+=f'CrestProcessor* make_{symbol}(float rate, size_t frames) {{\n    return new CrestPlugin(new {symbol}::{cls}(reinterpret_cast<void*>(1)), rate, frames, {str(instrument).lower()});\n}}\n'
  if name == 'ButterComp2': text=text.replace('../../vendor/audio/airwindows/plugins/WinVST/ButterComp2/','crest_butter/')
  (out/(symbol+'.cpp')).write_text(text)
  entries.append((symbol,instrument,family+' '+name))
entries.append(('mda_DeEsser',False,'mda DeEsser'))
(out/'plugin_factories.inc').write_text(''.join(f'CREST_FACTORY({s}, {str(i).lower()}, "{label}")\n' for s,i,label in entries))
