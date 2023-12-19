#include "VSyncThread.h"

#include <chrono>
#include <thread>

#include "Utils.h"
#include "Logger.h"

VSyncThread::VSyncThread(int refreshRate)
	: m_bExit(false)
	, m_refreshRate(refreshRate) {}

// Trigger VSync if elapsed time from previous VSync is larger than 30ms.
void VSyncThread::Run() {
	m_PreviousVsync = 0;

	while (!m_bExit) {
		const std::uint64_t current = GetSteadyTimeStampUS();
		const std::uint64_t interval = 1000 * 1000 / m_refreshRate;

		if (m_PreviousVsync + interval > current) {
			const std::uint64_t sleepTimeUs = (m_PreviousVsync + interval - current);

			if (sleepTimeUs > 0) {
				Debug("Sleep %llu us for next VSync.\n", sleepTimeUs);
				std::this_thread::sleep_for(std::chrono::microseconds(sleepTimeUs));
			}

			m_PreviousVsync += interval;
		}
		else {
			m_PreviousVsync = current;
		}
		Debug("Generate VSync Event by VSyncThread\n");
		vr::VRServerDriverHost()->VsyncEvent(0);
	}
}

void VSyncThread::Shutdown() {
	m_bExit = true;
}

void VSyncThread::SetRefreshRate(int refreshRate) {
	m_refreshRate = refreshRate;
}
