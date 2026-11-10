package main

import (
	"bytes"
	"encoding/binary"
	"testing"
)

func TestMachOBoundsBeforeParser(t *testing.T) {
	valid := make([]byte, 80)
	put := func(data []byte, offset int, value uint32) {
		binary.LittleEndian.PutUint32(data[offset:offset+4], value)
	}
	put(valid, 0, 0xfeedfacf)
	put(valid, 16, 1)
	put(valid, 20, 24)
	put(valid, 32, 2)
	put(valid, 36, 24)
	put(valid, 40, 56)
	put(valid, 44, 1)
	put(valid, 48, 72)
	put(valid, 52, 8)
	if err := validateMachOBounds(valid); err != nil {
		t.Fatal("有界符号表应通过预检", err)
	}
	for name, change := range map[string]struct {
		offset int
		value  uint32
	}{
		"command_count": {16, 0xffffffff}, "command_bytes": {20, 0xffffffff}, "command_length": {36, 0xfffffff8},
		"symbol_count": {44, 0xffffffff}, "symbol_offset": {40, 0xffffffff}, "string_size": {52, 0xffffffff},
		"string_offset": {48, 0xffffffff}, "symbol_name_offset": {56, 0xffffffff},
	} {
		t.Run(name, func(t *testing.T) {
			data := bytes.Clone(valid)
			put(data, change.offset, change.value)
			if err := validateMachOBounds(data); err == nil {
				t.Fatal("畸形Mach-O表范围应在标准库读取前拒绝")
			}
		})
	}
	unterminated := bytes.Clone(valid)
	copy(unterminated[72:], bytes.Repeat([]byte("x"), 8))
	if err := validateMachOBounds(unterminated); err == nil {
		t.Fatal("符号名缺少有界终止符应拒绝")
	}
	segment := make([]byte, 104)
	put(segment, 0, 0xfeedfacf)
	put(segment, 16, 1)
	put(segment, 20, 72)
	put(segment, 32, 0x19)
	put(segment, 36, 72)
	put(segment, 96, 0xffffffff)
	if err := validateMachOBounds(segment); err == nil {
		t.Fatal("段节数量应有界")
	}
	dynamic := make([]byte, 112)
	put(dynamic, 0, 0xfeedfacf)
	put(dynamic, 16, 1)
	put(dynamic, 20, 80)
	put(dynamic, 32, 0xb)
	put(dynamic, 36, 80)
	put(dynamic, 92, 0xffffffff)
	if err := validateMachOBounds(dynamic); err == nil {
		t.Fatal("间接符号表数量应有界")
	}
}
