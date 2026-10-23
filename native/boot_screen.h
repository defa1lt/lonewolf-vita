#ifndef LONEWOLF_BOOT_SCREEN_H
#define LONEWOLF_BOOT_SCREEN_H

/* Shared by Vita and the offscreen desktop preview. Coordinates are 960 x 544. */
typedef struct {
    GLuint program, vao, buffer, background, logo;
    GLint color, textured, key_black;
} BootScreen;

static void boot_destroy(BootScreen *screen) {
    if (screen->background) glDeleteTextures(1, &screen->background);
    if (screen->logo) glDeleteTextures(1, &screen->logo);
    if (screen->buffer) glDeleteBuffers(1, &screen->buffer);
    if (screen->vao) glDeleteVertexArrays(1, &screen->vao);
    if (screen->program) glDeleteProgram(screen->program);
    memset(screen, 0, sizeof(*screen));
}

static GLuint boot_shader(GLenum type, const char *source) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &source, NULL);
    glCompileShader(shader);
    GLint valid = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &valid);
    if (!valid) { glDeleteShader(shader); return 0; }
    return shader;
}

static int boot_create(BootScreen *screen) {
#ifdef __vita__
    const char *vertex = "#version 100\nattribute vec2 position; attribute vec2 uv; varying mediump vec2 texcoord;"
        "void main(){gl_Position=vec4(position.x/480.0-1.0,1.0-position.y/272.0,0.0,1.0);texcoord=uv;}";
    const char *fragment = "#version 100\nprecision mediump float; varying mediump vec2 texcoord;"
        "uniform sampler2D image; uniform vec4 color; uniform int textured; uniform int key_black;"
        "void main(){vec4 t=texture2D(image,texcoord);if(key_black==1&&all(lessThan(t.rgb,vec3(0.047))))t=vec4(0.0);gl_FragColor=textured==1?t*color:color;}";
#else
    const char *vertex = "#version 150\nin vec2 position; in vec2 uv; out vec2 texcoord;"
        "void main(){gl_Position=vec4(position.x/480.0-1.0,1.0-position.y/272.0,0.0,1.0);texcoord=uv;}";
    const char *fragment = "#version 150\nin vec2 texcoord; out vec4 output_color;"
        "uniform sampler2D image; uniform vec4 color; uniform int textured; uniform int key_black;"
        "void main(){vec4 t=texture(image,texcoord);if(key_black==1&&all(lessThan(t.rgb,vec3(0.047))))t=vec4(0.0);output_color=textured==1?t*color:color;}";
#endif
    GLuint vs = boot_shader(GL_VERTEX_SHADER, vertex), fs = boot_shader(GL_FRAGMENT_SHADER, fragment);
    if (!vs || !fs) { if (vs) glDeleteShader(vs); if (fs) glDeleteShader(fs); return -1; }
    screen->program = glCreateProgram();
    glAttachShader(screen->program, vs); glAttachShader(screen->program, fs);
    glBindAttribLocation(screen->program, 0, "position"); glBindAttribLocation(screen->program, 1, "uv");
    glLinkProgram(screen->program); glDeleteShader(vs); glDeleteShader(fs);
    GLint valid = 0; glGetProgramiv(screen->program, GL_LINK_STATUS, &valid);
    if (!valid) { boot_destroy(screen); return -1; }
    glUseProgram(screen->program);
    glUniform1i(glGetUniformLocation(screen->program, "image"), 0);
    screen->color = glGetUniformLocation(screen->program, "color");
    screen->textured = glGetUniformLocation(screen->program, "textured");
    screen->key_black = glGetUniformLocation(screen->program, "key_black");
    glGenVertexArrays(1, &screen->vao); glBindVertexArray(screen->vao);
    glGenBuffers(1, &screen->buffer); glBindBuffer(GL_ARRAY_BUFFER, screen->buffer);
    glEnableVertexAttribArray(0); glEnableVertexAttribArray(1);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 4 * sizeof(float), (void *)0);
    glVertexAttribPointer(1, 2, GL_FLOAT, GL_FALSE, 4 * sizeof(float), (void *)(2 * sizeof(float)));
    return 0;
}

static GLuint boot_texture(const unsigned char *pixels, unsigned int width, unsigned int height) {
    GLuint texture = 0; glGenTextures(1, &texture);
    glActiveTexture(GL_TEXTURE0); glBindTexture(GL_TEXTURE_2D, texture);
    /* RGBA rows are already four-byte aligned; VitaGL rejects GL_UNPACK_ALIGNMENT. */
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, width, height, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    if (glGetError() != GL_NO_ERROR) { glDeleteTextures(1, &texture); return 0; }
    return texture;
}

static void boot_quad(float x, float y, float w, float h, float top, float bottom) {
    const float vertices[] = { x,y,0,top, x,y+h,0,bottom, x+w,y,1,top, x+w,y+h,1,bottom };
    glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STREAM_DRAW);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
}

static void boot_draw(BootScreen *screen, unsigned int completed) {
    if (completed > 6) completed = 6;
    glViewport(0, 0, 960, 544); glDisable(GL_DEPTH_TEST); glDisable(GL_STENCIL_TEST);
    glDisable(GL_SCISSOR_TEST); glDisable(GL_CULL_FACE); glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
    glUseProgram(screen->program); glBindVertexArray(screen->vao); glBindBuffer(GL_ARRAY_BUFFER, screen->buffer);
    glActiveTexture(GL_TEXTURE0); glDisable(GL_BLEND);
    glUniform1i(screen->key_black, 0);
    glUniform1i(screen->textured, 1); glUniform4f(screen->color, 1,1,1,1);
    glBindTexture(GL_TEXTURE_2D, screen->background);
    boot_quad(0,0,960,544, 0.024f,0.976f);
    glEnable(GL_BLEND); glBlendEquation(GL_FUNC_ADD); glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
    glUniform1i(screen->key_black, 1);
    glBindTexture(GL_TEXTURE_2D, screen->logo);
    boot_quad(48,280,352,199,0,1);
    glDisable(GL_BLEND); glUniform1i(screen->textured, 0); glUniform1i(screen->key_black, 0);
    glUniform4f(screen->color, 0.32f,0.32f,0.30f,1);
    boot_quad(56,470,336,6,0,1);
    glUniform4f(screen->color, 0.87f,0.84f,0.76f,1);
    if (completed) boot_quad(56,470,336.0f*completed/6.0f,6,0,1);
    glBindVertexArray(0); glBindBuffer(GL_ARRAY_BUFFER, 0);
}
#endif
