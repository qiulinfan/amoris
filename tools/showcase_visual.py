#!/usr/bin/env python3
"""Record actual native renderer frames along a declared camera path, not a real-time FPS benchmark."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess

from showcase_record import http_call, encode

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("name")
    parser.add_argument("--host",default="http://127.0.0.1:8769")
    parser.add_argument("--path",type=Path,required=True,help="JSON list of camera position/look_at/fov_deg keyframes")
    parser.add_argument("--frames",type=int,default=240)
    parser.add_argument("--width",type=int,default=1920)
    parser.add_argument("--height",type=int,default=1080)
    args=parser.parse_args()
    cameras=json.loads(args.path.read_text())
    if len(cameras)<2:raise ValueError("At least two camera keyframes needed")
    frames=ROOT/"out/showcase"/f"{args.name}-frames";frames.mkdir(parents=True,exist_ok=True)
    media=ROOT/"site/media";media.mkdir(exist_ok=True)
    trace=[]
    for index in range(args.frames):
        u=index/(args.frames-1)*(len(cameras)-1)
        section=min(int(u),len(cameras)-2);v=u-section
        v=(1-math.cos(v*math.pi))/2
        a,b=cameras[section:section+2]
        camera={k:[x+(y-x)*v for x,y in zip(a[k],b[k])] for k in ["position","look_at"]}
        camera["fov_deg"]=a.get("fov_deg",44)+(b.get("fov_deg",44)-a.get("fov_deg",44))*v
        http_call(args.host,"time.step",{"ticks":2})
        capture=http_call(args.host,"capture",{**camera,"width":args.width,"height":args.height})
        if capture["pending_assets"]:raise RuntimeError("Capture has pending assets")
        shutil.copy2(capture["path"],frames/f"{index:05d}.png")
        trace.append({"frame":index,"camera":camera,**capture})
        if index%60==0:print(f"{args.name}: {index}/{args.frames}",flush=True)
    video=media/f"{args.name}.mp4";encode(frames,video,30)
    subprocess.run(["ffmpeg","-hide_banner","-loglevel","error","-y","-i",str(video),"-frames:v","1","-q:v","2","-update","1",str(media/f"{args.name}-poster.jpg")],check=True)
    receipt={"name":args.name,"kind":"actual_native_capture","fps":30,"frames":args.frames,"dimensions":[args.width,args.height],"duration_seconds":args.frames/30,"sha256":hashlib.sha256(video.read_bytes()).hexdigest(),"camera_path_sha256":hashlib.sha256(args.path.read_bytes()).hexdigest(),"recording_script_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),"timing":"Fixed 2-tick simulation steps; encoded playback rate is not measured rendering FPS."}
    (ROOT/"out/showcase"/f"{args.name}-receipt.json").write_text(json.dumps(receipt,indent=2)+"\n")
    (ROOT/"out/showcase"/f"{args.name}-trace.json").write_text(json.dumps(trace,indent=2)+"\n")
    print(json.dumps(receipt),flush=True)


if __name__=="__main__":main()
