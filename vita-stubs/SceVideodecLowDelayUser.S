.arch armv7a

.section .vitalink.fstubs.SceVideodecLowDelayUser,"ax",%progbits

	.align 4
	.global sceAvcdecSetLowDelayModeNongameapp_ld
	.type sceAvcdecSetLowDelayModeNongameapp_ld, %function
sceAvcdecSetLowDelayModeNongameapp_ld:
.if GEN_WEAK_EXPORTS
	.word 0x00000008
.else
	.word 0x00000000
.endif
	.word 0x6354EEFF
	.word 0xD57F82C1
	.align 4