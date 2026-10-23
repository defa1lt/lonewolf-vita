#include <psp2/ctrl.h>
#include <psp2/audioout.h>
#include <psp2/io/stat.h>
#include <psp2/io/fcntl.h>
#include <psp2/kernel/processmgr.h>
#include <psp2/power.h>
#include <psp2/touch.h>
#include <psp2/motion.h>
#include <stdio.h>
#include <malloc.h>
#include <string.h>
#include <vitaGL.h>
#include "boot_screen.h"

static BootScreen boot;

unsigned int _newlib_heap_size_user = 240 * 1024 * 1024;

typedef struct {
    unsigned int id;
    float x;
    float y;
} LonewolfTouch;

typedef struct {
    unsigned int buttons;
    float analog_x;
    float analog_y;
    float right_x;
    float right_y;
    unsigned int touch_count;
    LonewolfTouch touches[SCE_TOUCH_MAX_REPORT];
    /* Viewport yaw/pitch rates, in radians per second. */
    float gyro_x;
    float gyro_y;
} LonewolfInput;

_Static_assert(SCE_TOUCH_MAX_REPORT == 8 && sizeof(LonewolfInput) == 128, "Touch input ABI mismatch");

static SceTouchPanelInfo panel;
static int motion_ready;
static int motion_owned;

/* Match Gangfort: prevent VitaGL's logo thread without forking the library. */
void __wrap_invoke_splashscreen(void) {
}

int lonewolf_vita_audio_open(int frames, int sample_rate) {
    return sceAudioOutOpenPort(SCE_AUDIO_OUT_PORT_TYPE_MAIN, frames, sample_rate,
        SCE_AUDIO_OUT_MODE_STEREO);
}

int lonewolf_vita_audio_output(int port, const short *samples) {
    return sceAudioOutOutput(port, samples);
}

int lonewolf_vita_audio_drain(int port) {
    return sceAudioOutOutput(port, NULL);
}

int lonewolf_vita_audio_close(int port) {
    return sceAudioOutReleasePort(port);
}

static SceUID log_fd = -1;

int lonewolf_vita_log_init(void) {
    sceIoMkdir("ux0:data/lonewolf", 0777);
    log_fd = sceIoOpen("ux0:data/lonewolf/runtime.log", SCE_O_WRONLY | SCE_O_CREAT | SCE_O_APPEND, 0666);
    return log_fd < 0 ? log_fd : 0;
}

int lonewolf_vita_log(const char *data, unsigned int length) {
    if (log_fd < 0) return -1;
    unsigned int written = 0;
    while (written < length) {
        int count = sceIoWrite(log_fd, data + written, length - written);
        if (count <= 0) return -1;
        written += count;
    }
    return 0;
}

int lonewolf_vita_log_flush(void) {
    return log_fd < 0 ? -1 : sceIoSyncByFd(log_fd, 0);
}

void lonewolf_vita_log_memory(const char *stage) {
    struct mallinfo info = mallinfo();
    char message[384];
    int length = snprintf(message, sizeof(message),
        "[memory] %s elapsed_ms=%llu heap_used=%u heap_limit=%u GPU_RAM_free=%u GPU_VRAM_free=%u GPU_PHY_free=%u arena=%u heap_free=%u heap_top=%u heap_free_blocks=%u\n",
        stage, (unsigned long long)(sceKernelGetProcessTimeWide() / 1000),
        (unsigned int)info.uordblks, _newlib_heap_size_user,
        (unsigned int)vglMemFree(VGL_MEM_RAM), (unsigned int)vglMemFree(VGL_MEM_VRAM),
        (unsigned int)vglMemFree(VGL_MEM_PHYCONT),
        (unsigned int)info.arena, (unsigned int)info.fordblks,
        (unsigned int)info.keepcost, (unsigned int)info.ordblks);
    if (length > 0) lonewolf_vita_log(message, (unsigned int)length < sizeof(message) ? (unsigned int)length : sizeof(message) - 1);
}

void lonewolf_vita_exit(int status) {
    lonewolf_vita_log_flush();
    /* Newlib exit frees its heap before stopping VitaGL's native threads. */
    sceKernelExitProcess(status);
    for (;;) {}
}

int lonewolf_vita_loading_init(const unsigned char *background, unsigned int bw, unsigned int bh,
        const unsigned char *logo, unsigned int lw, unsigned int lh) {
    if (boot_create(&boot) != 0) return -4;
    boot.background = boot_texture(background, bw, bh);
    boot.logo = boot_texture(logo, lw, lh);
    if (!boot.background || !boot.logo) { boot_destroy(&boot); return -5; }
    return 0;
}

void lonewolf_vita_loading(unsigned int completed) {
    glBindFramebuffer(GL_FRAMEBUFFER, 0);
    boot_draw(&boot, completed);
    vglSwapBuffers(GL_TRUE);
}

void lonewolf_vita_loading_finish(void) {
    /* The preceding loading frame can still reference these textures on the GPU. */
    glFinish();
    boot_destroy(&boot);
}

int lonewolf_vita_init(void) {
    const char message[] = "[boot] initializing VitaGL\n";
    lonewolf_vita_log(message, sizeof(message) - 1);
    scePowerSetArmClockFrequency(444);
    scePowerSetBusClockFrequency(222);
    scePowerSetGpuClockFrequency(222);
    if (sceCtrlSetSamplingMode(SCE_CTRL_MODE_ANALOG) < 0) return -2;
    if (sceTouchGetPanelInfo(SCE_TOUCH_PORT_FRONT, &panel) < 0) return -3;
    if (panel.maxAaX <= panel.minAaX || panel.maxAaY <= panel.minAaY) return -3;
    if (sceTouchSetSamplingState(SCE_TOUCH_PORT_FRONT, SCE_TOUCH_SAMPLING_STATE_START) < 0) return -3;
    int motion_result = sceMotionStartSampling();
    motion_owned = motion_result >= 0;
    motion_ready = motion_owned || (unsigned int)motion_result == SCE_MOTION_ERROR_ALREADY_SAMPLING;
    if (!motion_ready) {
        const char warning[] = "[motion] unavailable; sticks and touch remain active\n";
        lonewolf_vita_log(warning, sizeof(warning) - 1);
    }
    /* The return value reports resolution fallback, not initialization success. */
    vglInitExtended(0, 960, 544, 4 * 1024 * 1024, SCE_GXM_MULTISAMPLE_NONE);
    const char ready[] = "[boot] VitaGL initialized\n";
    lonewolf_vita_log(ready, sizeof(ready) - 1);
    lonewolf_vita_log_memory("graphics ready");
    return 0;
}

void *lonewolf_gl_proc(const char *name) {
    return vglGetProcAddress(name);
}

int lonewolf_vita_poll(LonewolfInput *input) {
    SceCtrlData controls = {0};
    controls.lx = 128;
    controls.ly = 128;
    controls.rx = 128;
    controls.ry = 128;
    SceTouchData touch = {0};
    memset(input, 0, sizeof(*input));
    if (sceCtrlPeekBufferPositive(0, &controls, 1) < 0) return -1;
    input->buttons = controls.buttons;
    if (motion_ready && (controls.buttons & SCE_CTRL_LTRIGGER)) {
        SceMotionState state;
        if (sceMotionGetState(&state) == 0) {
            input->gyro_x = -state.angularVelocity.y;
            input->gyro_y = -state.angularVelocity.x;
        }
    }

    input->analog_x = ((float)controls.lx - 128.0f) / 128.0f;
    input->analog_y = ((float)controls.ly - 128.0f) / 128.0f;
    input->right_x = ((float)controls.rx - 128.0f) / 128.0f;
    input->right_y = ((float)controls.ry - 128.0f) / 128.0f;
    if (sceTouchPeek(SCE_TOUCH_PORT_FRONT, &touch, 1) > 0 && touch.reportNum > 0) {
        input->touch_count = touch.reportNum < SCE_TOUCH_MAX_REPORT ? touch.reportNum : SCE_TOUCH_MAX_REPORT;
        for (unsigned int i = 0; i < input->touch_count; ++i) {
            input->touches[i].id = touch.report[i].id;
            input->touches[i].x = (touch.report[i].x - panel.minAaX) * 960.0f / (panel.maxAaX - panel.minAaX);
            input->touches[i].y = (touch.report[i].y - panel.minAaY) * 544.0f / (panel.maxAaY - panel.minAaY);
        }
    }
    return 0;
}

void lonewolf_vita_present(float cursor_x, float cursor_y, int show_cursor) {
    if (show_cursor) {
        int x = (int)cursor_x;
        int y = 543 - (int)cursor_y;
        glEnable(GL_SCISSOR_TEST);
        glClearColor(1.0f, 1.0f, 1.0f, 1.0f);
        glScissor(x - 4, y, 9, 1);
        glClear(GL_COLOR_BUFFER_BIT);
        glScissor(x, y - 4, 1, 9);
        glClear(GL_COLOR_BUFFER_BIT);
        glDisable(GL_SCISSOR_TEST);
    }
    vglSwapBuffers(GL_TRUE);
}

void lonewolf_vita_finish(void) {
    if (motion_owned) sceMotionStopSampling();
    sceTouchSetSamplingState(SCE_TOUCH_PORT_FRONT, SCE_TOUCH_SAMPLING_STATE_STOP);
    glFinish();
    boot_destroy(&boot);
}
