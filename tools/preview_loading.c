/* macOS offscreen preview of the same renderer used by Vita. */
#include <OpenGL/OpenGL.h>
#include <OpenGL/gl3.h>
#include <ApplicationServices/ApplicationServices.h>
#include <ImageIO/ImageIO.h>
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../native/boot_screen.h"

static CFURLRef file_url(const char *path) {
    return CFURLCreateFromFileSystemRepresentation(NULL, (const UInt8 *)path, strlen(path), false);
}

static GLuint load_texture(const char *path) {
    CFURLRef url = file_url(path);
    CGImageSourceRef source = CGImageSourceCreateWithURL(url, NULL);
    assert(source);
    CGImageRef image = CGImageSourceCreateImageAtIndex(source, 0, NULL);
    assert(image);
    size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
    unsigned char *pixels = calloc(width * height, 4);
    assert(pixels);
    CGColorSpaceRef colors = CGColorSpaceCreateDeviceRGB();
    CGContextRef context = CGBitmapContextCreate(pixels, width, height, 8, width * 4, colors,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    assert(context);
    CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
    GLuint texture = boot_texture(pixels, (unsigned)width, (unsigned)height);
    assert(texture);
    CGContextRelease(context); CGColorSpaceRelease(colors); free(pixels);
    CGImageRelease(image); CFRelease(source); CFRelease(url);
    return texture;
}

static void save_frame(const char *path, unsigned int stage) {
    unsigned char *pixels = malloc(960 * 544 * 4), row[960 * 4];
    assert(pixels);
    glReadPixels(0, 0, 960, 544, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    assert(glGetError() == GL_NO_ERROR);
    for (unsigned y = 0; y < 272; ++y) {
        unsigned char *top = pixels + y * sizeof(row), *bottom = pixels + (543-y) * sizeof(row);
        memcpy(row, top, sizeof(row)); memcpy(top, bottom, sizeof(row)); memcpy(bottom, row, sizeof(row));
    }
    /* Every completed stage contributes exactly 56 pixels to the real bar. */
    for (unsigned x = 56; x < 392; ++x) {
        const unsigned char *pixel = pixels + (472 * 960 + x) * 4;
        unsigned char expected = x < 56 + 56 * stage ? 222 : 82;
        assert(abs((int)pixel[0] - expected) <= 1);
    }
    CGColorSpaceRef colors = CGColorSpaceCreateDeviceRGB();
    CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, pixels, 960 * 544 * 4, NULL);
    CGImageRef image = CGImageCreate(960, 544, 8, 32, 960 * 4, colors,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big, provider, NULL, false, kCGRenderingIntentDefault);
    CFURLRef url = file_url(path);
    CGImageDestinationRef output = CGImageDestinationCreateWithURL(url, CFSTR("public.png"), 1, NULL);
    assert(output); CGImageDestinationAddImage(output, image, NULL);
    assert(CGImageDestinationFinalize(output));
    CFRelease(output); CFRelease(url); CGImageRelease(image);
    CGDataProviderRelease(provider); CGColorSpaceRelease(colors); free(pixels);
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "Usage: %s bg0.png startup.png output-directory\n", argv[0]);
        return 2;
    }
    CGLPixelFormatAttribute attributes[] = {
        kCGLPFAOpenGLProfile, (CGLPixelFormatAttribute)kCGLOGLPVersion_3_2_Core,
        kCGLPFAAccelerated, kCGLPFAColorSize, (CGLPixelFormatAttribute)32,
        (CGLPixelFormatAttribute)0
    };
    CGLPixelFormatObj format; GLint count;
    assert(CGLChoosePixelFormat(attributes, &format, &count) == kCGLNoError);
    CGLContextObj context;
    assert(CGLCreateContext(format, NULL, &context) == kCGLNoError);
    assert(CGLSetCurrentContext(context) == kCGLNoError);
    BootScreen screen = {0}; assert(boot_create(&screen) == 0);
    assert(glGetError() == GL_NO_ERROR);
    screen.background = load_texture(argv[1]); screen.logo = load_texture(argv[2]);
    GLuint target, framebuffer;
    glGenTextures(1, &target); glBindTexture(GL_TEXTURE_2D, target);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, 960, 544, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    glGenFramebuffers(1, &framebuffer); glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, target, 0);
    assert(glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE);
    for (unsigned stage = 0; stage <= 6; ++stage) {
        boot_draw(&screen, stage); glFinish();
        char path[4096]; snprintf(path, sizeof(path), "%s/loading-stage-%u.png", argv[3], stage);
        save_frame(path, stage);
        printf("Verified stage %u/6: %s\n", stage, path);
    }
    boot_destroy(&screen); glDeleteFramebuffers(1, &framebuffer); glDeleteTextures(1, &target);
    assert(glGetError() == GL_NO_ERROR);
    CGLSetCurrentContext(NULL); CGLDestroyContext(context); CGLDestroyPixelFormat(format);
    return 0;
}
