// Minimal stub for WebKit's wtf/Vector.h
// Bao runs SpiderMonkey, not JSC, so there is no WebKit WTF to include:
// upstream bun resolves <wtf/Vector.h> from its WebKit SDK include paths.
// This provides exactly the surface bun-uws headers use (a growable
// contiguous buffer: append / grow / shrink / span / range-for); extend as
// absorbed headers start calling more of it — do not grow it speculatively.

#ifndef WTF_VECTOR_H
#define WTF_VECTOR_H

#include <cassert>
#include <cstddef>
#include <span>
#include <utility>

namespace WTF {

template <typename T, size_t InlineCapacity = 0>
class Vector {
public:
    Vector() = default;

    ~Vector() { clear(); }

    Vector(const Vector&) = delete;
    Vector& operator=(const Vector&) = delete;

    Vector(Vector&& other) noexcept
        : m_data(other.m_data)
        , m_size(other.m_size)
        , m_capacity(other.m_capacity)
    {
        other.m_data = nullptr;
        other.m_size = 0;
        other.m_capacity = 0;
    }

    Vector& operator=(Vector&& other) noexcept
    {
        if (this != &other) {
            clear();
            m_data = std::exchange(other.m_data, nullptr);
            m_size = std::exchange(other.m_size, 0u);
            m_capacity = std::exchange(other.m_capacity, 0u);
        }
        return *this;
    }

    void append(T value)
    {
        grow(m_size + 1);
        m_data[m_size - 1] = std::move(value);
    }

    void append(std::span<const T> values)
    {
        if (values.empty()) {
            return;
        }
        size_t start = m_size;
        grow(m_size + values.size());
        for (size_t i = 0; i < values.size(); ++i) {
            m_data[start + i] = values[i];
        }
    }

    /* WebKit WTF semantics: grow the size to `size`, keeping existing bytes.
     * The new tail is uninitialized from the container's point of view (the
     * callers overwrite it right after growing). */
    void grow(size_t size)
    {
        if (size > m_capacity) {
            size_t newCapacity = m_capacity ? m_capacity : 16;
            while (newCapacity < size) {
                newCapacity *= 2;
            }
            T* newData = new T[newCapacity];
            for (size_t i = 0; i < m_size; ++i) {
                newData[i] = std::move(m_data[i]);
            }
            delete[] m_data;
            m_data = newData;
            m_capacity = newCapacity;
        }
        m_size = size;
    }

    /* Truncate to `size` bytes, keeping the capacity. */
    void shrink(size_t size)
    {
        assert(size <= m_size);
        m_size = size;
    }

    size_t size() const { return m_size; }
    bool isEmpty() const { return m_size == 0; }
    T* data() { return m_data; }
    const T* data() const { return m_data; }

    std::span<T> mutableSpan() { return { m_data, m_size }; }
    std::span<const T> span() const { return { m_data, m_size }; }

    T* begin() { return m_data; }
    T* end() { return m_data + m_size; }
    const T* begin() const { return m_data; }
    const T* end() const { return m_data + m_size; }

    T& operator[](size_t index)
    {
        assert(index < m_size);
        return m_data[index];
    }

    const T& operator[](size_t index) const
    {
        assert(index < m_size);
        return m_data[index];
    }

    void clear()
    {
        delete[] m_data;
        m_data = nullptr;
        m_size = 0;
        m_capacity = 0;
    }

private:
    T* m_data = nullptr;
    size_t m_size = 0;
    size_t m_capacity = 0;
};

} // namespace WTF

#endif // WTF_VECTOR_H
