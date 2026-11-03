#!/usr/bin/env python3
"""用已安装 Go / 固定缓存模块生成公开容器向量；不下载、不接收用户秘密。"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


SOURCE = r'''package main

import (
    "bytes"
    "crypto/hkdf"
    "crypto/sha256"
    "encoding/binary"
    "encoding/hex"
    "encoding/json"
    "fmt"
    "os"
    "golang.org/x/crypto/argon2"
    "golang.org/x/crypto/chacha20poly1305"
)

func must(err error) { if err != nil { panic("public vector oracle failed") } }
func raw(s string) []byte { b,e:=hex.DecodeString(s);must(e);return b }
func put(b *bytes.Buffer, value any) { must(binary.Write(b,binary.BigEndian,value)) }
func derive(ikm,salt []byte,info string) []byte { k,e:=hkdf.Key(sha256.New,ikm,salt,info,32);must(e);return k }
func aead(key,nonce,input,aad []byte) []byte { c,e:=chacha20poly1305.NewX(key);must(e);return c.Seal(nil,nonce,input,aad) }
func digest(b []byte) string { d:=sha256.Sum256(b);return hex.EncodeToString(d[:]) }

func main() {
    input,e:=os.ReadFile(os.Args[1]);must(e)
    var fixture struct {
        DataOrigin string `json:"dataOrigin"`
        Base struct {
            Password string `json:"passwordUtf8"`
            ArchiveKey string `json:"archiveKeyHex"`
            ID string `json:"archiveIdHex"`
            Salt string `json:"saltHex"`
            WrapNonce string `json:"wrapNonceHex"`
            BodyNonce string `json:"contentNonceHex"`
            Payload string `json:"payloadUtf8"`
        } `json:"baseInput"`
    }
    must(json.Unmarshal(input,&fixture))
    if fixture.DataOrigin!="synthetic-public-review-inputs" || fixture.Base.Password!="public-review-password" { panic("only public fixture permitted") }
    b:=fixture.Base;id,salt:=raw(b.ID),raw(b.Salt);payload:=[]byte(b.Payload)
    // 独立按提案重建，不消费 framingExpected 的头、清单或 hash。
    var header bytes.Buffer
    header.WriteString("RLM0E001")
    put(&header,uint16(0));put(&header,uint16(1));put(&header,uint32(176))
    put(&header,uint16(1));put(&header,uint16(1));put(&header,uint32(0))
    header.Write(id);header.Write(salt)
    for _,n:=range []uint32{19,65536,3,1} {put(&header,n)}
    header.Write(raw(b.WrapNonce));header.Write(raw(b.BodyNonce));put(&header,uint64(len(payload)+152))
    var content struct { Facts struct {
        SourceSchema uint32 `json:"sourceSchema"`;Sequence uint64 `json:"sequence"`
        Accounts,Categories,Revisions,Selected,Candidates []json.RawMessage
        Intents []json.RawMessage `json:"intentHistory"`
        Imports *struct {Tasks,Rows,Sources,Batches,UndoTasks,UndoRows,UndoBatches []json.RawMessage} `json:"imports"`
    } `json:"facts"` }
    must(json.Unmarshal(payload,&content));f:=content.Facts
    counts:=[]uint32{uint32(len(f.Accounts)),uint32(len(f.Categories)),uint32(len(f.Revisions)),uint32(len(f.Selected)),uint32(len(f.Candidates)),uint32(len(f.Intents))}
    if f.Imports==nil {counts=append(counts,make([]uint32,7)...)} else {panic("baseline vector must be the declared empty fixture")}
    var manifest bytes.Buffer
    manifest.WriteString("RLM0MAN1");put(&manifest,uint16(1));put(&manifest,uint16(1));put(&manifest,f.SourceSchema);put(&manifest,f.Sequence)
    for _,n:=range counts {put(&manifest,n)}
    put(&manifest,uint64(len(payload)));d:=sha256.Sum256(payload);manifest.Write(d[:])
    var plaintext bytes.Buffer
    put(&plaintext,uint32(manifest.Len()));plaintext.Write(manifest.Bytes());put(&plaintext,uint64(len(payload)));plaintext.Write(payload);plaintext.WriteString("RLM0END1")
    ikm:=argon2.IDKey([]byte(b.Password),salt,3,65536,1,32)
    wrap:=derive(ikm,id,"radishledger/archive/review1/password-wrap")
    body:=derive(raw(b.ArchiveKey),id,"radishledger/archive/review1/content-key")
    wrapAAD:=append([]byte("radishledger/archive/review1/wrap\x00"),header.Bytes()...)
    sealedKey:=aead(wrap,raw(b.WrapNonce),raw(b.ArchiveKey),wrapAAD)
    header.Write(sealedKey)
    bodyAAD:=append([]byte("radishledger/archive/review1/content\x00"),header.Bytes()...)
    sealedBody:=aead(body,raw(b.BodyNonce),plaintext.Bytes(),bodyAAD)
    archive:=append(append([]byte{},header.Bytes()...),sealedBody...)
    out:=map[string]any{
        "status":"independent_go_oracle",
        "argon2idOutputHex":hex.EncodeToString(ikm),"wrapKeyHex":hex.EncodeToString(wrap),"contentKeyHex":hex.EncodeToString(body),
        "wrappedArchiveKeyHex":hex.EncodeToString(sealedKey),"contentCiphertextHex":hex.EncodeToString(sealedBody),
        "archiveSha256":digest(archive),"archiveHex":hex.EncodeToString(archive),
        "m0cPasswordKeyHex":hex.EncodeToString(derive(ikm,salt,"radishledger/m0c/v1/password-wrap")),
        "unicodeNfcArgon2Hex":hex.EncodeToString(argon2.IDKey([]byte("\u00e9"),salt,3,65536,1,32)),
        "unicodeNfdArgon2Hex":hex.EncodeToString(argon2.IDKey([]byte("e\u0301"),salt,3,65536,1,32)),
        "headerPrefixHex":hex.EncodeToString(header.Bytes()[:128]),"manifestHex":hex.EncodeToString(manifest.Bytes()),"plaintextSha256":digest(plaintext.Bytes()),
    }
    encoded,e:=json.MarshalIndent(out,"","  ");must(e);fmt.Println(string(encoded))
}
'''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    evidence = (root / "local-evidence/m0-e").resolve()
    target = Path(args.work_dir).resolve()
    if not target.is_relative_to(evidence) or target == evidence or target.exists():
        parser.error("work-dir must be a new exclusive directory under local-evidence/m0-e")
    material = root / "docs/validation/archive-container-review-inputs.json"
    data = json.loads(material.read_text())
    base = data["baseInput"]
    if data["dataOrigin"] != "synthetic-public-review-inputs" or base["passwordUtf8"] != "public-review-password" or bytes.fromhex(base["archiveKeyHex"]) != bytes(range(128,160)):
        parser.error("only the fixed public fixture is permitted")
    go = shutil.which("go")
    if not go:
        parser.error("installed Go required; no installation attempted")
    cache = Path.home() / "go/pkg/mod/golang.org/x"
    crypto = cache / "crypto@v0.50.0"
    system = cache / "sys@v0.43.0"
    if not all((p / "go.mod").is_file() for p in (crypto,system)):
        parser.error("fixed cached Go modules required; no download attempted")
    target.mkdir(parents=True, exist_ok=False)
    (target / "main.go").write_text(SOURCE)
    (target / "go.mod").write_text(
        "module radishledger-public-vector-oracle\n\ngo 1.26.0\n\n"
        "require (\n golang.org/x/crypto v0.50.0\n golang.org/x/sys v0.43.0\n)\n\n"
        f"replace golang.org/x/crypto => {crypto}\nreplace golang.org/x/sys => {system}\n"
    )
    env = {**os.environ,"GOPROXY":"off","GOSUMDB":"off","GOTOOLCHAIN":"local","GOWORK":"off","GOENV":"off","GOTELEMETRY":"off","GOCACHE":str(target/"cache"),"GOMAXPROCS":"2"}
    subprocess.run([go,"build","-mod=readonly","-p","2","-o",str(target/"oracle"),"."],cwd=target,env=env,check=True,timeout=120)
    result = subprocess.run([str(target/"oracle"),str(material)],cwd=target,env=env,check=True,capture_output=True,timeout=20)
    output = json.loads(result.stdout)
    for key in ("headerPrefixHex","manifestHex","plaintextSha256"):
        if output[key] != data["framingExpected"][key]:
            raise ValueError("independent framing mismatch")
    (target/"outputs.json").write_text(json.dumps(output,indent=2)+"\n")
    provenance = {"goVersion":subprocess.check_output([go,"version"],env=env,text=True).strip(),"modules":{"golang.org/x/crypto":"v0.50.0","golang.org/x/sys":"v0.43.0"},"license":"BSD-3-Clause; The Go Authors","sourceSha256":hashlib.sha256(SOURCE.encode()).hexdigest(),"materialSha256":hashlib.sha256(material.read_bytes()).hexdigest(),"outputsSha256":hashlib.sha256(result.stdout).hexdigest(),"network":"GOPROXY=off / GOSUMDB=off / GOTOOLCHAIN=local","packageSources":{}}
    for module in (crypto,system):
        files = sorted(module.rglob("*.go"))
        digest=hashlib.sha256()
        for file in files:
            digest.update(file.relative_to(module).as_posix().encode()+b"\0"+file.read_bytes())
        provenance["packageSources"][module.name] = {"goSourceCount":len(files),"sha256":digest.hexdigest(),"licenseSha256":hashlib.sha256((module/"LICENSE").read_bytes()).hexdigest()}
    (target/"provenance.json").write_text(json.dumps(provenance,indent=2)+"\n")
    print("independent Go oracle complete; public outputs and provenance:",target)


if __name__ == "__main__":
    main()
