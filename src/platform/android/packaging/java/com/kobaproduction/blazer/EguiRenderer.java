package com.kobaproduction.blazer;

import static android.opengl.GLES20.*;

import android.opengl.GLSurfaceView;
import java.nio.BufferUnderflowException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

/** GLES2 renderer for EGR1, the Rust bridge's little-endian egui frame packet. */
final class EguiRenderer implements GLSurfaceView.Renderer {
    private static final int MAGIC = 0x31524745;
    private static final int VERTEX_BYTES = 20;
    private static final int MAX_FRAME_BYTES = 32 * 1024 * 1024;
    private static final String VERTEX_SHADER = """
        attribute vec2 position;
        attribute vec2 textureUv;
        attribute vec4 color;
        uniform vec2 size;
        varying mediump vec2 uv;
        varying lowp vec4 tint;
        void main() {
            gl_Position = vec4(2.0 * position.x / size.x - 1.0,
                               1.0 - 2.0 * position.y / size.y, 0.0, 1.0);
            uv = textureUv;
            tint = color;
        }
        """;
    private static final String FRAGMENT_SHADER = """
        precision mediump float;
        uniform sampler2D atlas;
        varying mediump vec2 uv;
        varying lowp vec4 tint;
        void main() {
            gl_FragColor = tint * texture2D(atlas, uv);
        }
        """;
    private static final int POSITION = 0;
    private static final int UV = 1;
    private static final int COLOR = 2;

    private final EguiSurface surface;
    private final Map<Long, Texture> textures = new HashMap<>();
    private ByteBuffer data = ByteBuffer.allocateDirect(0).order(ByteOrder.LITTLE_ENDIAN);
    private int program;
    private int screenSize;
    private int vertexBuffer;
    private int indexBuffer;
    private int maxTextureSize;
    private int width;
    private int height;
    private boolean failed;

    EguiRenderer(EguiSurface surface) {
        this.surface = surface;
    }

    @Override public void onSurfaceCreated(GL10 gl, EGLConfig config) {
        textures.clear();
        int[] limit = new int[1];
        glGetIntegerv(GL_MAX_TEXTURE_SIZE, limit, 0);
        maxTextureSize = limit[0];
        program = createProgram();
        int[] buffers = new int[2];
        glGenBuffers(2, buffers, 0);
        vertexBuffer = buffers[0];
        indexBuffer = buffers[1];
        if (program == 0 || maxTextureSize <= 0 || vertexBuffer == 0 || indexBuffer == 0) {
            fail();
            return;
        }
        failed = false;
        glUseProgram(program);
        screenSize = glGetUniformLocation(program, "size");
        glUniform1i(glGetUniformLocation(program, "atlas"), 0);
        glEnableVertexAttribArray(POSITION);
        glEnableVertexAttribArray(UV);
        glEnableVertexAttribArray(COLOR);
        glBindBuffer(GL_ARRAY_BUFFER, vertexBuffer);
        glVertexAttribPointer(POSITION, 2, GL_FLOAT, false, VERTEX_BYTES, 0);
        glVertexAttribPointer(UV, 2, GL_FLOAT, false, VERTEX_BYTES, 8);
        glVertexAttribPointer(COLOR, 4, GL_UNSIGNED_BYTE, true, VERTEX_BYTES, 16);
        glBindBuffer(GL_ELEMENT_ARRAY_BUFFER, indexBuffer);
        glActiveTexture(GL_TEXTURE0);
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        glEnable(GL_BLEND);
        glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
        glDisable(GL_CULL_FACE);
        glDisable(GL_DEPTH_TEST);
        EguiNative.surfaceReset(surface.handle());
    }

    @Override public void onSurfaceChanged(GL10 gl, int width, int height) {
        this.width = width;
        this.height = height;
        glViewport(0, 0, width, height);
    }

    @Override public void onDrawFrame(GL10 gl) {
        if (failed || program == 0 || width <= 0 || height <= 0) return;
        float density = surface.getResources().getDisplayMetrics().density;
        byte[] frame = EguiNative.render(surface.handle(), width, height, density, System.nanoTime());
        glDisable(GL_SCISSOR_TEST);
        glClearColor(0.067f, 0.067f, 0.067f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        if (frame == null) return;
        if (frame.length > MAX_FRAME_BYTES) {
            fail();
            return;
        }
        if (data.capacity() < frame.length) {
            data = ByteBuffer.allocateDirect(frame.length).order(ByteOrder.LITTLE_ENDIAN);
        }
        data.clear();
        data.put(frame).flip();
        try {
            renderPacket();
        } catch (IllegalArgumentException | BufferUnderflowException invalid) {
            fail();
        }
    }

    private void renderPacket() {
        require(data.getInt() == MAGIC);
        int updates = count(24);
        int meshes = count(32);
        int removals = count(8);
        int flags = data.getInt();
        String copied = readText();
        String url = readText();
        for (int i = 0; i < updates; i++) uploadTexture();
        glUniform2f(screenSize, width, height);
        glEnable(GL_SCISSOR_TEST);
        for (int i = 0; i < meshes; i++) drawMesh();
        glDisable(GL_SCISSOR_TEST);
        for (int i = 0; i < removals; i++) deleteTexture(data.getLong());
        require(!data.hasRemaining());
        surface.publishOutput(flags, copied, url);
    }

    private void uploadTexture() {
        long id = data.getLong();
        int x = data.getInt();
        int y = data.getInt();
        int columns = data.getInt();
        int rows = data.getInt();
        require(columns > 0 && columns <= maxTextureSize && rows > 0 && rows <= maxTextureSize);
        long bytes = (long) columns * rows * 4L;
        require(bytes <= data.remaining());
        ByteBuffer pixels = consume((int) bytes);
        if (x == -1 && y == -1) {
            deleteTexture(id);
            int[] names = new int[1];
            glGenTextures(1, names, 0);
            require(names[0] != 0);
            glBindTexture(GL_TEXTURE_2D, names[0]);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, columns, rows, 0, GL_RGBA, GL_UNSIGNED_BYTE,
                pixels);
            textures.put(id, new Texture(names[0], columns, rows));
        } else {
            Texture texture = textures.get(id);
            require(texture != null && x >= 0 && y >= 0 && x <= texture.width() - columns
                && y <= texture.height() - rows);
            glBindTexture(GL_TEXTURE_2D, texture.glId());
            glTexSubImage2D(GL_TEXTURE_2D, 0, x, y, columns, rows, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
        }
    }

    private void drawMesh() {
        long id = data.getLong();
        int left = clip(data.getFloat(), width);
        int top = clip(data.getFloat(), height);
        int right = clip(data.getFloat(), width);
        int bottom = clip(data.getFloat(), height);
        int vertexCount = count(VERTEX_BYTES);
        int indexCount = count(Short.BYTES);
        require(vertexCount <= 65536 && indexCount % 3 == 0);
        ByteBuffer vertices = consume(vertexCount * VERTEX_BYTES);
        ByteBuffer indices = consume(indexCount * Short.BYTES);
        Texture texture = textures.get(id);
        if (texture == null || indexCount == 0 || right <= left || bottom <= top) return;
        require(vertexCount > 0);
        glScissor(left, height - bottom, right - left, bottom - top);
        glBindTexture(GL_TEXTURE_2D, texture.glId());
        glBufferData(GL_ARRAY_BUFFER, vertices.remaining(), vertices, GL_STREAM_DRAW);
        glBufferData(GL_ELEMENT_ARRAY_BUFFER, indices.remaining(), indices, GL_STREAM_DRAW);
        glDrawElements(GL_TRIANGLES, indexCount, GL_UNSIGNED_SHORT, 0);
    }

    private void deleteTexture(long id) {
        Texture previous = textures.remove(id);
        if (previous != null) glDeleteTextures(1, new int[] {previous.glId()}, 0);
    }

    private void fail() {
        failed = true;
        surface.rendererFailed();
    }

    private int count(int bytesPerItem) {
        int count = data.getInt();
        require(count >= 0 && count <= data.remaining() / bytesPerItem);
        return count;
    }

    private String readText() {
        int bytes = count(1);
        if (bytes == 0) return "";
        return StandardCharsets.UTF_8.decode(consume(bytes)).toString();
    }

    private ByteBuffer consume(int bytes) {
        require(bytes >= 0 && bytes <= data.remaining());
        int start = data.position();
        ByteBuffer part = data.slice().order(ByteOrder.LITTLE_ENDIAN);
        part.limit(bytes);
        data.position(start + bytes);
        return part;
    }

    private static int clip(float coordinate, int boundary) {
        return Math.max(0, Math.min(Math.round(coordinate), boundary));
    }

    private static void require(boolean valid) {
        if (!valid) throw new IllegalArgumentException("Invalid egui frame packet");
    }

    private static int createProgram() {
        int vertex = compile(GL_VERTEX_SHADER, VERTEX_SHADER);
        int fragment = compile(GL_FRAGMENT_SHADER, FRAGMENT_SHADER);
        int linked = 0;
        if (vertex != 0 && fragment != 0) {
            linked = glCreateProgram();
            glAttachShader(linked, vertex);
            glAttachShader(linked, fragment);
            glBindAttribLocation(linked, POSITION, "position");
            glBindAttribLocation(linked, UV, "textureUv");
            glBindAttribLocation(linked, COLOR, "color");
            glLinkProgram(linked);
            int[] status = new int[1];
            glGetProgramiv(linked, GL_LINK_STATUS, status, 0);
            if (status[0] == 0) {
                glDeleteProgram(linked);
                linked = 0;
            }
        }
        if (vertex != 0) glDeleteShader(vertex);
        if (fragment != 0) glDeleteShader(fragment);
        return linked;
    }

    private static int compile(int type, String source) {
        int shader = glCreateShader(type);
        if (shader == 0) return 0;
        glShaderSource(shader, source);
        glCompileShader(shader);
        int[] status = new int[1];
        glGetShaderiv(shader, GL_COMPILE_STATUS, status, 0);
        if (status[0] != 0) return shader;
        glDeleteShader(shader);
        return 0;
    }

    private record Texture(int glId, int width, int height) {}
}
